use crate::capture::EncoderSettings;
use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::unix::io::AsRawFd;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::watch;
use tokio_tungstenite::accept_async;
use tokio_tungstenite::tungstenite::Message;
use tracing::{error, info, warn};

// Linux input event constants
const EV_SYN: u16 = 0x00;
const EV_KEY: u16 = 0x01;
const EV_ABS: u16 = 0x03;

const SYN_REPORT: u16 = 0x00;

const BTN_TOUCH: u16 = 0x14a;
const BTN_TOOL_FINGER: u16 = 0x145;
const BTN_TOOL_PEN: u16 = 0x140;
const BTN_TOOL_RUBBER: u16 = 0x141;

const ABS_X: u16 = 0x00;
const ABS_Y: u16 = 0x01;
const ABS_PRESSURE: u16 = 0x18;
const ABS_MT_SLOT: u16 = 0x2f;
const ABS_MT_POSITION_X: u16 = 0x35;
const ABS_MT_POSITION_Y: u16 = 0x36;
const ABS_MT_TRACKING_ID: u16 = 0x39;
const ABS_MT_PRESSURE: u16 = 0x3a;
const ABS_TILT_X: u16 = 0x1a;
const ABS_TILT_Y: u16 = 0x1b;

const BTN_STYLUS: u16 = 0x14b;

// uinput ioctl constants (modern UI_DEV_SETUP/UI_ABS_SETUP API — the legacy
// uinput_user_dev write() API cannot declare axis resolution, which makes
// libinput reject the device: "missing tablet capabilities ... resolution")
const UI_SET_EVBIT: libc::c_ulong = 0x40045564;
const UI_SET_KEYBIT: libc::c_ulong = 0x40045565;
const UI_SET_ABSBIT: libc::c_ulong = 0x40045567;
const UI_SET_PROPBIT: libc::c_ulong = 0x4004556e;
const UI_DEV_SETUP: libc::c_ulong = 0x405c5503;
const UI_ABS_SETUP: libc::c_ulong = 0x401c5504;
const UI_DEV_CREATE: libc::c_ulong = 0x5501;
const UI_DEV_DESTROY: libc::c_ulong = 0x5502;

const BUS_VIRTUAL: u16 = 0x06;
const INPUT_PROP_DIRECT: i32 = 0x01;

/// Coordinates are injected in a fixed 0..65535 space — the compositor maps
/// the device onto the output, so the virtual display resolution can change
/// at runtime without recreating uinput devices.
const COORD_MAX: i32 = 65535;

/// Identity of the virtual input devices. These must stay stable: KDE keys the
/// device→output association in kcminputrc on vendor/product/name, so changing
/// any of them silently orphans the mapping written by `map_devices_to_output`.
const UINPUT_VENDOR: u16 = 0x4553;
const PRODUCT_TOUCH: u16 = 0x0001;
const PRODUCT_PEN: u16 = 0x0002;
const TOUCH_DEVICE_NAME: &str = "UScreen Touch";
const PEN_DEVICE_NAME: &str = "UScreen Pen";
/// ~310mm wide active area → 65535/310 ≈ 211 units/mm.
/// libinput requires a resolution on touchscreen/tablet axes.
const RESOLUTION_UNITS_PER_MM: i32 = 211;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct InputAbsInfo {
    value: i32,
    minimum: i32,
    maximum: i32,
    fuzz: i32,
    flat: i32,
    resolution: i32,
}

#[repr(C)]
struct UinputAbsSetup {
    code: u16,
    _pad: u16,
    absinfo: InputAbsInfo,
}

#[repr(C)]
struct UinputSetup {
    bustype: u16,
    vendor: u16,
    product: u16,
    version: u16,
    name: [u8; 80],
    ff_effects_max: u32,
}

#[derive(Deserialize, Debug)]
#[serde(tag = "type")]
pub enum InputEvent {
    #[serde(rename = "touch")]
    Touch {
        x: f64,
        y: f64,
        pressure: f64,
        action: u8,
        slot: u8,
    },
    #[serde(rename = "pen")]
    Pen {
        x: f64,
        y: f64,
        pressure: f64,
        tilt_x: f64,
        tilt_y: f64,
        /// True when the pen's eraser end is in use (TOOL_TYPE_ERASER).
        /// Emitted as BTN_TOOL_RUBBER so GIMP's eraser works.
        #[serde(default)]
        eraser: bool,
        /// 0=down 1=up 2=move 3=hover 4=hover_exit
        /// 5=stylus button down 6=stylus button up
        action: u8,
    },
    #[serde(rename = "resolution")]
    Resolution {
        width: u32,
        height: u32,
        /// Physical panel size, when the tablet knows it. Feeds the EDID so
        /// the desktop derives the right DPI and default scale.
        #[serde(default)]
        width_mm: u32,
        #[serde(default)]
        height_mm: u32,
    },
    /// Settings pushed from the tablet app's settings UI
    #[serde(rename = "config")]
    Config {
        bitrate: Option<u32>,
        fps: Option<u32>,
        encoder: Option<String>,
    },
    /// The tablet has this frame on screen. Closes the latency measurement
    /// loop — the host timed the frame out, so the round trip needs no clock
    /// agreement between the two devices.
    #[serde(rename = "rendered")]
    Rendered {
        seq: u32,
        /// Microseconds the tablet spent between receiving the frame and
        /// putting it on screen. Subtracting it from the round trip isolates
        /// what the transport actually costs.
        #[serde(default)]
        decode_us: i64,
    },
}

#[derive(Serialize)]
pub struct InputResponse {
    pub status: String,
    pub width: u32,
    pub height: u32,
    /// Tells the tablet not to expect a video stream: it is acting as a
    /// graphics tablet for the host's own screen, not as a display.
    pub pen_only: bool,
}

#[derive(Clone)]
pub struct InputConfig {
    pub port: u16,
    /// Map the devices onto the laptop's screen rather than the virtual one.
    pub pen_only: bool,
    pub virtual_width: u32,
    pub virtual_height: u32,
}

impl Default for InputConfig {
    fn default() -> Self {
        Self {
            port: 8891,
            pen_only: false,
            virtual_width: 2960,
            virtual_height: 1848,
        }
    }
}

// Linux input_event struct (for writing to uinput)
#[repr(C, packed)]
#[derive(Clone, Copy)]
struct LinuxInputEvent {
    tv_sec: i64,
    tv_usec: i64,
    type_: u16,
    code: u16,
    value: i32,
}

/// A uinput virtual input device for injecting touch/pen events into Linux.
/// Touch and pen are SEPARATE devices: libinput classifies a touchscreen and
/// a tablet pen differently and rejects a device that mixes both.
struct UInputDevice {
    file: File,
}

impl UInputDevice {
    fn open_uinput() -> Result<File> {
        OpenOptions::new()
            .write(true)
            .open("/dev/uinput")
            .context("Failed to open /dev/uinput. Ensure the uinput module is loaded and you have permissions (try: sudo modprobe uinput)")
    }

    /// Touchscreen device: multitouch + single-touch axes, INPUT_PROP_DIRECT.
    fn new_touch(name: &str) -> Result<Self> {
        let file = Self::open_uinput()?;
        let fd = file.as_raw_fd();
        let w = COORD_MAX + 1;
        let h = COORD_MAX + 1;

        unsafe {
            Self::ioctl_val(fd, UI_SET_EVBIT, EV_SYN as i32)?;
            Self::ioctl_val(fd, UI_SET_EVBIT, EV_KEY as i32)?;
            Self::ioctl_val(fd, UI_SET_EVBIT, EV_ABS as i32)?;
            Self::ioctl_val(fd, UI_SET_PROPBIT, INPUT_PROP_DIRECT)?;

            Self::ioctl_val(fd, UI_SET_KEYBIT, BTN_TOUCH as i32)?;
            Self::ioctl_val(fd, UI_SET_KEYBIT, BTN_TOOL_FINGER as i32)?;

            Self::abs_setup(fd, ABS_X, 0, w - 1, RESOLUTION_UNITS_PER_MM)?;
            Self::abs_setup(fd, ABS_Y, 0, h - 1, RESOLUTION_UNITS_PER_MM)?;
            Self::abs_setup(fd, ABS_PRESSURE, 0, 4096, 0)?;
            Self::abs_setup(fd, ABS_MT_SLOT, 0, 9, 0)?;
            Self::abs_setup(fd, ABS_MT_POSITION_X, 0, w - 1, RESOLUTION_UNITS_PER_MM)?;
            Self::abs_setup(fd, ABS_MT_POSITION_Y, 0, h - 1, RESOLUTION_UNITS_PER_MM)?;
            Self::abs_setup(fd, ABS_MT_TRACKING_ID, 0, 65535, 0)?;
            Self::abs_setup(fd, ABS_MT_PRESSURE, 0, 4096, 0)?;

            Self::dev_setup_and_create(fd, name, PRODUCT_TOUCH)?;
        }

        info!("uinput touchscreen '{}' created", name);
        std::thread::sleep(std::time::Duration::from_millis(200));
        Ok(Self { file })
    }

    /// Pen tablet device: stylus tool + pressure + tilt.
    /// No INPUT_PROP_DIRECT — that flag means "touchscreen" and causes KDE to
    /// activate the on-screen keyboard on every pen tap. Without it, libinput
    /// classifies this as a tablet tool (Wacom-style): the cursor follows the
    /// pen position and clicks work as mouse clicks.
    fn new_pen(name: &str) -> Result<Self> {
        let file = Self::open_uinput()?;
        let fd = file.as_raw_fd();
        let w = COORD_MAX + 1;
        let h = COORD_MAX + 1;

        unsafe {
            Self::ioctl_val(fd, UI_SET_EVBIT, EV_SYN as i32)?;
            Self::ioctl_val(fd, UI_SET_EVBIT, EV_KEY as i32)?;
            Self::ioctl_val(fd, UI_SET_EVBIT, EV_ABS as i32)?;

            Self::ioctl_val(fd, UI_SET_KEYBIT, BTN_TOUCH as i32)?;
            Self::ioctl_val(fd, UI_SET_KEYBIT, BTN_TOOL_PEN as i32)?;
            // Eraser end: reported as TOOL_TYPE_ERASER on Android, mapped to
            // BTN_TOOL_RUBBER here so GIMP's eraser tool follows the pen.
            Self::ioctl_val(fd, UI_SET_KEYBIT, BTN_TOOL_RUBBER as i32)?;
            // libinput requires the stylus button capability on pen devices
            Self::ioctl_val(fd, UI_SET_KEYBIT, BTN_STYLUS as i32)?;

            Self::abs_setup(fd, ABS_X, 0, w - 1, RESOLUTION_UNITS_PER_MM)?;
            Self::abs_setup(fd, ABS_Y, 0, h - 1, RESOLUTION_UNITS_PER_MM)?;
            Self::abs_setup(fd, ABS_PRESSURE, 0, 4096, 0)?;
            // Tilt in whole degrees
            Self::abs_setup(fd, ABS_TILT_X, -90, 90, 0)?;
            Self::abs_setup(fd, ABS_TILT_Y, -90, 90, 0)?;

            Self::dev_setup_and_create(fd, name, PRODUCT_PEN)?;
        }

        info!("uinput pen tablet '{}' created", name);
        std::thread::sleep(std::time::Duration::from_millis(200));
        Ok(Self { file })
    }

    unsafe fn abs_setup(fd: i32, code: u16, min: i32, max: i32, resolution: i32) -> Result<()> {
        let setup = UinputAbsSetup {
            code,
            _pad: 0,
            absinfo: InputAbsInfo {
                value: 0,
                minimum: min,
                maximum: max,
                fuzz: 0,
                flat: 0,
                resolution,
            },
        };
        Self::ioctl_val(fd, UI_SET_ABSBIT, code as i32)?;
        if libc::ioctl(fd, UI_ABS_SETUP, &setup as *const UinputAbsSetup) < 0 {
            anyhow::bail!(
                "UI_ABS_SETUP({:#x}) failed: {}",
                code,
                std::io::Error::last_os_error()
            );
        }
        Ok(())
    }

    unsafe fn dev_setup_and_create(fd: i32, name: &str, product: u16) -> Result<()> {
        let mut setup = UinputSetup {
            bustype: BUS_VIRTUAL,
            vendor: UINPUT_VENDOR,
            product,
            version: 1,
            name: [0u8; 80],
            ff_effects_max: 0,
        };
        let name_bytes = name.as_bytes();
        let len = name_bytes.len().min(79);
        setup.name[..len].copy_from_slice(&name_bytes[..len]);

        if libc::ioctl(fd, UI_DEV_SETUP, &setup as *const UinputSetup) < 0 {
            anyhow::bail!("UI_DEV_SETUP failed: {}", std::io::Error::last_os_error());
        }
        if libc::ioctl(fd, UI_DEV_CREATE) < 0 {
            anyhow::bail!("UI_DEV_CREATE failed: {}", std::io::Error::last_os_error());
        }
        Ok(())
    }

    unsafe fn ioctl_val(fd: i32, request: libc::c_ulong, value: i32) -> Result<()> {
        if libc::ioctl(fd, request as libc::c_ulong, value) < 0 {
            anyhow::bail!(
                "ioctl({:#x}, {}) failed: {}",
                request,
                value,
                std::io::Error::last_os_error()
            );
        }
        Ok(())
    }

    fn emit(&mut self, type_: u16, code: u16, value: i32) -> Result<()> {
        let ev = LinuxInputEvent {
            tv_sec: 0,
            tv_usec: 0,
            type_,
            code,
            value,
        };
        let bytes: &[u8] = unsafe {
            std::slice::from_raw_parts(
                &ev as *const LinuxInputEvent as *const u8,
                std::mem::size_of::<LinuxInputEvent>(),
            )
        };
        self.file.write_all(bytes)?;
        Ok(())
    }

    fn syn(&mut self) -> Result<()> {
        self.emit(EV_SYN, SYN_REPORT, 0)?;
        self.file.flush()?;
        Ok(())
    }

    fn inject_touch(&mut self, x: i32, y: i32, pressure: i32, action: u8, slot: u8) -> Result<()> {
        match action {
            0 => {
                // DOWN
                self.emit(EV_ABS, ABS_MT_SLOT, slot as i32)?;
                self.emit(EV_ABS, ABS_MT_TRACKING_ID, slot as i32)?;
                self.emit(EV_ABS, ABS_MT_POSITION_X, x)?;
                self.emit(EV_ABS, ABS_MT_POSITION_Y, y)?;
                self.emit(EV_ABS, ABS_MT_PRESSURE, pressure)?;
                self.emit(EV_KEY, BTN_TOUCH, 1)?;
                self.emit(EV_KEY, BTN_TOOL_FINGER, 1)?;
                self.emit(EV_ABS, ABS_X, x)?;
                self.emit(EV_ABS, ABS_Y, y)?;
                self.emit(EV_ABS, ABS_PRESSURE, pressure)?;
                self.syn()?;
            }
            1 => {
                // UP
                self.emit(EV_ABS, ABS_MT_SLOT, slot as i32)?;
                self.emit(EV_ABS, ABS_MT_TRACKING_ID, -1)?;
                self.emit(EV_KEY, BTN_TOUCH, 0)?;
                self.emit(EV_KEY, BTN_TOOL_FINGER, 0)?;
                self.emit(EV_ABS, ABS_PRESSURE, 0)?;
                self.syn()?;
            }
            2 => {
                // MOVE - combine with previous if possible
                self.emit(EV_ABS, ABS_MT_SLOT, slot as i32)?;
                self.emit(EV_ABS, ABS_MT_POSITION_X, x)?;
                self.emit(EV_ABS, ABS_MT_POSITION_Y, y)?;
                self.emit(EV_ABS, ABS_MT_PRESSURE, pressure)?;
                self.emit(EV_ABS, ABS_X, x)?;
                self.emit(EV_ABS, ABS_Y, y)?;
                self.emit(EV_ABS, ABS_PRESSURE, pressure)?;
                self.syn()?;
            }
            _ => {}
        }
        Ok(())
    }

    fn inject_pen(
        &mut self,
        x: i32,
        y: i32,
        pressure: i32,
        tilt_x: i32,
        tilt_y: i32,
        action: u8,
        eraser: bool,
    ) -> Result<()> {
        // The active tablet-tool key depends on which end of the pen is in
        // use. An S Pen flipped to its eraser end reports TOOL_TYPE_ERASER.
        let tool = if eraser { BTN_TOOL_RUBBER } else { BTN_TOOL_PEN };
        match action {
            0 => {
                // DOWN, in two frames.
                //
                // A tablet tool has to enter proximity before it can touch:
                // libinput wants to see the tool appear at a position, and only
                // then the tip come down. Announcing both in a single event
                // frame makes the first tap after picking up the pen land at
                // the previous cursor position, or get dropped entirely.
                self.emit(EV_KEY, tool, 1)?;
                self.emit(EV_ABS, ABS_X, x)?;
                self.emit(EV_ABS, ABS_Y, y)?;
                self.emit(EV_ABS, ABS_TILT_X, tilt_x)?;
                self.emit(EV_ABS, ABS_TILT_Y, tilt_y)?;
                self.syn()?;

                self.emit(EV_KEY, BTN_TOUCH, 1)?;
                self.emit(EV_ABS, ABS_PRESSURE, pressure)?;
                self.syn()?;
            }
            1 => {
                // UP
                self.emit(EV_KEY, BTN_TOUCH, 0)?;
                self.emit(EV_KEY, tool, 0)?;
                self.emit(EV_ABS, ABS_PRESSURE, 0)?;
                self.syn()?;
            }
            2 => {
                // MOVE (pressing)
                self.emit(EV_ABS, ABS_X, x)?;
                self.emit(EV_ABS, ABS_Y, y)?;
                self.emit(EV_ABS, ABS_PRESSURE, pressure)?;
                self.emit(EV_ABS, ABS_TILT_X, tilt_x)?;
                self.emit(EV_ABS, ABS_TILT_Y, tilt_y)?;
                self.syn()?;
            }
            3 => {
                // HOVER — pen near screen, cursor follows without clicking.
                // Requires no INPUT_PROP_DIRECT on the device (we removed it)
                // so libinput classifies this as a tablet tool in proximity.
                self.emit(EV_KEY, tool, 1)?;
                self.emit(EV_ABS, ABS_X, x)?;
                self.emit(EV_ABS, ABS_Y, y)?;
                self.emit(EV_ABS, ABS_PRESSURE, 0)?;
                self.emit(EV_ABS, ABS_TILT_X, tilt_x)?;
                self.emit(EV_ABS, ABS_TILT_Y, tilt_y)?;
                self.syn()?;
            }
            4 => {
                // HOVER_EXIT — pen left proximity
                self.emit(EV_KEY, BTN_TOUCH, 0)?;
                self.emit(EV_KEY, BTN_TOOL_PEN, 0)?;
                self.emit(EV_KEY, BTN_TOOL_RUBBER, 0)?;
                self.emit(EV_ABS, ABS_PRESSURE, 0)?;
                self.syn()?;
            }
            5 => {
                // STYLUS BUTTON DOWN (S Pen side button → right-click in GIMP)
                self.emit(EV_KEY, BTN_STYLUS, 1)?;
                self.syn()?;
            }
            6 => {
                // STYLUS BUTTON UP
                self.emit(EV_KEY, BTN_STYLUS, 0)?;
                self.syn()?;
            }
            _ => {}
        }
        Ok(())
    }
}

impl Drop for UInputDevice {
    fn drop(&mut self) {
        unsafe {
            let fd = self.file.as_raw_fd();
            libc::ioctl(fd, UI_DEV_DESTROY as libc::c_ulong);
        }
        info!("uinput device destroyed");
    }
}

/// The pair of virtual input devices backing one tablet connection.
struct InjectDevices {
    touch: Option<UInputDevice>,
    pen: Option<UInputDevice>,
    /// Bitmask of MT slots that currently have an active tracking ID
    /// (DOWN received, no matching UP yet). Bit N → slot N, up to slot 15.
    active_slots: u16,
    pen_proximity: bool,
    /// S Pen side button currently held (BTN_STYLUS). Tracked so a held
    /// button is released cleanly if the connection drops.
    pen_button: bool,
}

impl InjectDevices {
    /// Release all active contacts cleanly before the connection closes.
    /// Without this, a stuck MT slot or a pen left in proximity causes
    /// the next connection to inherit phantom input events.
    fn release_all(&mut self) {
        if let Some(ref mut dev) = self.touch {
            for slot in 0u8..16 {
                if self.active_slots & (1u16 << slot) != 0 {
                    let _ = dev.emit(EV_ABS, ABS_MT_SLOT, slot as i32);
                    let _ = dev.emit(EV_ABS, ABS_MT_TRACKING_ID, -1);
                }
            }
            if self.active_slots != 0 {
                let _ = dev.emit(EV_KEY, BTN_TOUCH, 0);
                let _ = dev.emit(EV_KEY, BTN_TOOL_FINGER, 0);
                let _ = dev.syn();
            }
        }
        self.active_slots = 0;

        if self.pen_proximity {
            if let Some(ref mut dev) = self.pen {
                let _ = dev.emit(EV_KEY, BTN_TOUCH, 0);
                let _ = dev.emit(EV_KEY, BTN_TOOL_PEN, 0);
                let _ = dev.emit(EV_KEY, BTN_TOOL_RUBBER, 0);
                let _ = dev.emit(EV_ABS, ABS_PRESSURE, 0);
                let _ = dev.syn();
            }
        }
        self.pen_proximity = false;

        if self.pen_button {
            if let Some(ref mut dev) = self.pen {
                let _ = dev.emit(EV_KEY, BTN_STYLUS, 0);
                let _ = dev.syn();
            }
        }
        self.pen_button = false;
    }
}

const KWIN_INPUT_IFACE: &str = "org.kde.KWin.InputDevice";

/// The screen the user is actually looking at: the first enabled output that is
/// not one of ours. In pen-only mode the tablet drives this one, so the pen has
/// to be mapped onto it rather than onto the virtual display.
async fn primary_non_evdi_output() -> Option<String> {
    let evdi: Vec<String> = crate::vdisplay::evdi_connectors()
        .into_iter()
        .map(|c| c.name)
        .collect();
    let out = tokio::process::Command::new("kscreen-doctor")
        .arg("-j")
        .output()
        .await
        .ok()?;
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).ok()?;
    let outputs = v.get("outputs")?.as_array()?;
    let mut fallback = None;
    for o in outputs {
        let name = o.get("name").and_then(|v| v.as_str())?.to_string();
        if evdi.contains(&name) || !o.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false) {
            continue;
        }
        if o.get("primary").and_then(|v| v.as_bool()).unwrap_or(false) {
            return Some(name);
        }
        fallback.get_or_insert(name);
    }
    fallback
}

async fn kwin_device_property(sysname: &str, property: &str) -> Option<String> {
    let out = tokio::process::Command::new("qdbus")
        .args([
            "--literal",
            "org.kde.KWin",
            &format!("/org/kde/KWin/InputDevice/{}", sysname),
            "org.freedesktop.DBus.Properties.Get",
            KWIN_INPUT_IFACE,
            property,
        ])
        .output()
        .await
        .ok()?;
    if !out.status.success() {
        return None;
    }
    // qdbus --literal prints: [Variant(QString): "value"]
    let text = String::from_utf8_lossy(&out.stdout);
    let start = text.find('"')? + 1;
    let end = text.rfind('"')?;
    (end > start).then(|| text[start..end].to_string())
}

/// Pin the virtual input devices to the virtual display.
///
/// An absolute-positioning device is meaningless without knowing which screen
/// it addresses. Left unmapped, libinput spreads it across the whole desktop:
/// touching the middle of the tablet lands the cursor somewhere on the laptop
/// panel, and drawing with the pen goes to the wrong monitor entirely.
///
/// This is done over KWin's D-Bus interface rather than by writing kcminputrc.
/// Writing the config file looks like the obvious route and does produce the
/// documented `[Libinput][vendor][product][name] OutputName=` entry, but KWin
/// does not apply it to these devices — verified by reading the property back
/// and finding it empty, both when written before and after device creation.
/// Setting the property directly takes effect immediately, and KWin persists it
/// itself.
async fn map_devices_to_output(pen_only: bool) {
    let output = if pen_only {
        match primary_non_evdi_output().await {
            Some(name) => name,
            None => {
                warn!("No physical output found — pen will address the whole desktop");
                return;
            }
        }
    } else {
        let connectors = crate::vdisplay::evdi_connectors();
        match connectors
            .iter()
            .find(|c| c.connected)
            .or_else(|| connectors.first())
            .map(|c| c.name.clone())
        {
            Some(name) => name,
            None => {
                warn!("No EVDI output found — touch and pen will address the whole desktop");
                return;
            }
        }
    };

    // KWin registers a device slightly after uinput creates it, so retry
    // rather than racing it.
    for attempt in 0..20 {
        if attempt > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        }

        let Ok(list) = tokio::process::Command::new("qdbus")
            .args([
                "org.kde.KWin",
                "/org/kde/KWin/InputDevice",
                "org.kde.KWin.InputDeviceManager.devicesSysNames",
            ])
            .output()
            .await
        else {
            warn!("qdbus unavailable — input devices stay unmapped");
            return;
        };

        let mut mapped = 0;
        for sysname in String::from_utf8_lossy(&list.stdout)
            .split_whitespace()
            .map(str::to_string)
            .collect::<Vec<_>>()
        {
            let Some(name) = kwin_device_property(&sysname, "name").await else {
                continue;
            };
            if name != TOUCH_DEVICE_NAME && name != PEN_DEVICE_NAME {
                continue;
            }
            let r = tokio::process::Command::new("qdbus")
                .args([
                    "--literal",
                    "org.kde.KWin",
                    &format!("/org/kde/KWin/InputDevice/{}", sysname),
                    "org.freedesktop.DBus.Properties.Set",
                    KWIN_INPUT_IFACE,
                    "outputName",
                    &output,
                ])
                .output()
                .await;
            match r {
                Ok(o) if o.status.success() => {
                    info!("Mapped '{}' ({}) to output {}", name, sysname, output);
                    mapped += 1;
                }
                Ok(o) => warn!(
                    "Could not map '{}': {}",
                    name,
                    String::from_utf8_lossy(&o.stderr).trim()
                ),
                Err(e) => warn!("Could not map '{}': {}", name, e),
            }
        }

        if mapped >= 2 {
            return;
        }
    }

    warn!("Input devices did not appear in KWin within 5s — mapping skipped");
}

pub struct InputServer {
    config: InputConfig,
    running: Arc<AtomicBool>,
    settings_tx: Option<watch::Sender<EncoderSettings>>,
    latency: crate::latency::LatencyTracker,
}

impl InputServer {
    pub fn new(
        config: InputConfig,
        settings_tx: Option<watch::Sender<EncoderSettings>>,
        latency: crate::latency::LatencyTracker,
    ) -> Self {
        Self {
            config,
            running: Arc::new(AtomicBool::new(false)),
            settings_tx,
            latency,
        }
    }

    pub async fn run(&self) -> Result<()> {
        self.running.store(true, Ordering::SeqCst);
        // Loopback only — see the note in stream.rs. This socket injects real
        // mouse/pen/touch events into the desktop through uinput, so exposing
        // it on the network hands over control of the machine.
        let addr = format!("127.0.0.1:{}", self.config.port);

        let listener = TcpListener::bind(&addr)
            .await
            .context(format!("Failed to bind input server to {}", addr))?;

        info!("Input server on ws://{}", addr);

        // Created once for the lifetime of the daemon, not per connection.
        // Recreating them on every reconnect gave the devices a fresh identity
        // each time, so KDE reran input configuration and the output mapping
        // below had nothing stable to attach to.
        //
        // Device creation sleeps to let udev settle, so it runs off the async
        // runtime rather than blocking a worker thread.
        let devices = tokio::task::spawn_blocking(|| {
            let touch = match UInputDevice::new_touch(TOUCH_DEVICE_NAME) {
                Ok(dev) => Some(dev),
                Err(e) => {
                    warn!("No touch device: {}. Touch will be logged only.", e);
                    None
                }
            };
            let pen = match UInputDevice::new_pen(PEN_DEVICE_NAME) {
                Ok(dev) => Some(dev),
                Err(e) => {
                    warn!("No pen device: {}. Pen will be logged only.", e);
                    None
                }
            };
            InjectDevices {
                touch,
                pen,
                active_slots: 0,
                pen_proximity: false,
                pen_button: false,
            }
        })
        .await
        .unwrap_or(InjectDevices {
            touch: None,
            pen: None,
            active_slots: 0,
            pen_proximity: false,
            pen_button: false,
        });
        let uinput = Arc::new(std::sync::Mutex::new(devices));

        map_devices_to_output(self.config.pen_only).await;

        let config = self.config.clone();
        let running = self.running.clone();

        loop {
            let accept = tokio::select! {
                res = listener.accept() => res,
                _ = async {
                    while running.load(Ordering::SeqCst) {
                        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                    }
                } => break,
            };

            let (socket, peer) = match accept {
                Ok(s) => s,
                Err(e) => {
                    error!("Input accept failed: {}", e);
                    continue;
                }
            };

            info!("Input client: {}", peer);
            let cfg = config.clone();
            let settings = self.settings_tx.clone();
            let latency = self.latency.clone();
            let devices = uinput.clone();
            tokio::spawn(async move {
                if let Err(e) = handle_connection(socket, cfg, settings, latency, devices).await {
                    warn!("Input handler {}: {}", peer, e);
                }
            });
        }

        Ok(())
    }

    /// Counterpart to `run`; shutdown currently goes through task
    /// cancellation instead, but leaving this makes the lifecycle explicit.
    #[allow(dead_code)]
    pub fn stop(&self) {
        self.running.store(false, Ordering::SeqCst);
    }
}

async fn handle_connection(
    raw_stream: tokio::net::TcpStream,
    config: InputConfig,
    settings_tx: Option<watch::Sender<EncoderSettings>>,
    latency: crate::latency::LatencyTracker,
    uinput: Arc<std::sync::Mutex<InjectDevices>>,
) -> Result<()> {
    let ws_stream = accept_async(raw_stream)
        .await
        .context("WebSocket handshake failed")?;

    let (mut ws_sender, mut ws_receiver) = ws_stream.split();

    let resp = InputResponse {
        status: "connected".to_string(),
        width: config.virtual_width,
        height: config.virtual_height,
        pen_only: config.pen_only,
    };

    ws_sender
        .send(Message::Text(serde_json::to_string(&resp)?))
        .await?;

    while let Some(msg) = ws_receiver.next().await {
        match msg {
            Ok(Message::Text(text)) => match serde_json::from_str::<InputEvent>(&text) {
                Ok(event) => {
                    handle_event(event, &uinput, &settings_tx, &latency);
                }
                Err(e) => {
                    warn!("Invalid input: {} - {}", e, text);
                }
            },
            Ok(Message::Close(_)) | Err(_) => break,
            Ok(Message::Ping(data)) => {
                let _ = ws_sender.send(Message::Pong(data)).await;
            }
            _ => {}
        }
    }

    // Release any stuck MT slots or pen proximity. The devices themselves now
    // outlive the connection, so without this the next client inherits a
    // phantom finger or a pen stuck in proximity — which shows up as
    // unstoppable scrolling.
    if let Ok(mut guard) = uinput.lock() {
        guard.release_all();
    }

    Ok(())
}

fn handle_event(
    event: InputEvent,
    uinput: &Arc<std::sync::Mutex<InjectDevices>>,
    settings_tx: &Option<watch::Sender<EncoderSettings>>,
    latency: &crate::latency::LatencyTracker,
) {
    match event {
        InputEvent::Touch {
            x,
            y,
            pressure,
            action,
            slot,
        } => {
            let abs_x = (x * COORD_MAX as f64) as i32;
            let abs_y = (y * COORD_MAX as f64) as i32;
            let abs_pressure = (pressure * 4096.0) as i32;

            if let Ok(mut guard) = uinput.lock() {
                let ok = if let Some(ref mut dev) = guard.touch {
                    match dev.inject_touch(abs_x, abs_y, abs_pressure, action, slot) {
                        Ok(_) => true,
                        Err(e) => { warn!("Failed to inject touch: {}", e); false }
                    }
                } else {
                    match action {
                        0 => info!("Touch DOWN at ({}, {})", abs_x, abs_y),
                        1 => info!("Touch UP   at ({}, {})", abs_x, abs_y),
                        _ => {}
                    }
                    false
                };
                if ok {
                    let bit = 1u16 << (slot.min(15) as u16);
                    match action {
                        0 => guard.active_slots |= bit,
                        1 => guard.active_slots &= !bit,
                        _ => {}
                    }
                }
            }
        }
        InputEvent::Pen {
            x,
            y,
            pressure,
            tilt_x,
            tilt_y,
            eraser,
            action,
        } => {
            let abs_x = (x * COORD_MAX as f64) as i32;
            let abs_y = (y * COORD_MAX as f64) as i32;
            let abs_pressure = (pressure * 4096.0) as i32;
            // Already degrees, as the tablet computes them. This used to
            // multiply by 180/π on the assumption they were radians, which
            // squashed a pen laid flat at 90° down to 57°.
            let tilt_x_deg = (tilt_x.round() as i32).clamp(-90, 90);
            let tilt_y_deg = (tilt_y.round() as i32).clamp(-90, 90);

            if let Ok(mut guard) = uinput.lock() {
                let ok = if let Some(ref mut dev) = guard.pen {
                    match dev.inject_pen(abs_x, abs_y, abs_pressure, tilt_x_deg, tilt_y_deg, action, eraser) {
                        Ok(_) => true,
                        Err(e) => { warn!("Failed to inject pen: {}", e); false }
                    }
                } else {
                    match action {
                        0 => info!("Pen DOWN at ({}, {}), eraser={}, tilt=({:.1},{:.1})", abs_x, abs_y, eraser, tilt_x, tilt_y),
                        1 => info!("Pen UP   at ({}, {})", abs_x, abs_y),
                        _ => {}
                    }
                    false
                };
                if ok {
                    match action {
                        0 | 3 => guard.pen_proximity = true,
                        1 | 4 => guard.pen_proximity = false,
                        5 => guard.pen_button = true,
                        6 => guard.pen_button = false,
                        _ => {}
                    }
                }
            }
        }
        InputEvent::Resolution {
            width,
            height,
            width_mm,
            height_mm,
        } => {
            info!(
                "Tablet reports native resolution: {}x{} ({}x{} mm)",
                width, height, width_mm, height_mm
            );
            let Some(tx) = settings_tx else { return };
            if !crate::config::FileConfig::load().auto_resolution {
                info!("auto_resolution is off — keeping configured resolution");
                return;
            }
            if !(640..=8192).contains(&width) || !(480..=8192).contains(&height) {
                warn!("Ignoring implausible resolution {}x{}", width, height);
                return;
            }
            let mut new = tx.borrow().clone();
            // Reject nonsense physical sizes rather than baking them into an
            // EDID: a bad DPI makes the desktop come up at a absurd scale.
            let (mm_w, mm_h) = if (50..=1000).contains(&width_mm) && (50..=1000).contains(&height_mm)
            {
                (width_mm, height_mm)
            } else {
                (crate::edid::DEFAULT_WIDTH_MM, crate::edid::DEFAULT_HEIGHT_MM)
            };
            if new.width != width
                || new.height != height
                || new.width_mm != mm_w
                || new.height_mm != mm_h
            {
                new.width = width;
                new.height = height;
                new.width_mm = mm_w;
                new.height_mm = mm_h;
                info!(
                    "Auto-resolution: switching virtual display to {}x{} ({}x{} mm)",
                    width, height, mm_w, mm_h
                );
                let _ = tx.send(new);
            }
        }
        InputEvent::Rendered { seq, decode_us } => {
            latency.on_rendered(seq, decode_us);
        }
        InputEvent::Config {
            bitrate,
            fps,
            encoder,
        } => {
            let Some(tx) = settings_tx else {
                warn!("Received config from tablet but live settings are disabled");
                return;
            };
            let mut new = tx.borrow().clone();
            if let Some(b) = bitrate {
                // Clamped to the same ceiling the config file uses: an
                // unclamped value here would be persisted and poison every
                // later run, which is exactly how installs ended up pinned at
                // 200 Mbps with seconds of queueing delay.
                new.bitrate = b.clamp(
                    crate::config::MIN_BITRATE_KBPS,
                    crate::config::MAX_BITRATE_KBPS,
                );
                if new.bitrate != b {
                    warn!("Tablet asked for {} kbps — clamped to {}", b, new.bitrate);
                }
            }
            if let Some(f) = fps {
                new.fps = f.clamp(crate::config::MIN_FPS, crate::config::MAX_FPS);
            }
            if let Some(e) = encoder {
                new.encoder = e;
            }
            if *tx.borrow() != new {
                info!(
                    "Tablet pushed settings: encoder={} {}kbps @{}fps",
                    new.encoder, new.bitrate, new.fps
                );
                let _ = tx.send(new);
            }
        }
    }
}
