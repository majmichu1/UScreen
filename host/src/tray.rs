//! System tray icon.
//!
//! The daemon starts with the desktop, which means it otherwise runs with
//! nothing to show for itself: no way to see whether a tablet is attached, and
//! no way to change what it is doing without opening the GUI or a terminal.
//!
//! This is a StatusNotifierItem over D-Bus. On a Wayland session that is the
//! only tray protocol there is — XEmbed is X11-only — so there is no fallback
//! to write, and a desktop without a StatusNotifierWatcher simply gets no icon.
//! That is not an error worth failing the daemon over.

use ksni::menu::{CheckmarkItem, MenuItem, StandardItem};
use ksni::{Handle, Status, ToolTip, Tray, TrayMethods};
use tokio::sync::watch;
use tracing::{info, warn};

/// Theme icons rather than shipped assets, matching what uscreen.desktop
/// already uses. They also carry the mode at a glance without a tooltip.
const ICON_SCREEN: &str = "video-display";
const ICON_TABLET: &str = "input-tablet";

struct UScreenTray {
    pen_only: bool,
    tablet_present: bool,
    /// Newer release, when the daily check found one.
    update: Option<String>,
    mode_tx: watch::Sender<bool>,
    shutdown_tx: watch::Sender<bool>,
}

impl UScreenTray {
    fn state_line(&self) -> String {
        if !self.tablet_present {
            "No tablet connected".to_string()
        } else if self.pen_only {
            "Graphics tablet — the pen drives this screen".to_string()
        } else {
            "Second screen".to_string()
        }
    }
}

impl Tray for UScreenTray {
    fn id(&self) -> String {
        "uscreen".into()
    }

    fn title(&self) -> String {
        "UScreen".into()
    }

    fn icon_name(&self) -> String {
        if self.pen_only {
            ICON_TABLET.into()
        } else {
            ICON_SCREEN.into()
        }
    }

    /// Always visible. Passive lets the desktop hide the icon, and an icon you
    /// have to go looking for defeats the point of adding one.
    fn status(&self) -> Status {
        Status::Active
    }

    fn tool_tip(&self) -> ToolTip {
        let mut description = self.state_line();
        if let Some(v) = &self.update {
            description.push_str(&format!("\nUpdate available: {}", v));
        }
        ToolTip {
            icon_name: self.icon_name(),
            title: "UScreen".into(),
            description,
            ..Default::default()
        }
    }

    /// Left click opens the settings window, which is what a tray icon for a
    /// background service is normally expected to do.
    fn activate(&mut self, _x: i32, _y: i32) {
        open_settings();
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        let mut items: Vec<MenuItem<Self>> = vec![
            StandardItem {
                label: self.state_line(),
                enabled: false,
                ..Default::default()
            }
            .into(),
        ];
        if let Some(v) = &self.update {
            items.push(
                StandardItem {
                    label: format!("Update available: {} — open release page", v),
                    icon_name: "system-software-update".into(),
                    activate: Box::new(|_: &mut Self| open_release_page()),
                    ..Default::default()
                }
                .into(),
            );
        }
        items.extend([
            MenuItem::Separator,
            CheckmarkItem {
                label: "Graphics tablet".into(),
                checked: self.pen_only,
                // Settable with no tablet attached. The mode is a persistent
                // choice, not an action on a live device: it decides what the
                // tablet will be the moment it is plugged in, so greying it
                // out would take away the one chance to set it in advance.
                activate: Box::new(|t: &mut Self| {
                    let want = !t.pen_only;
                    // The mode channel is the source of truth. Do not flip the
                    // local copy here: the update task below sets it from the
                    // channel, so the menu can only ever show a mode the
                    // daemon actually entered.
                    let _ = t.mode_tx.send(want);
                }),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: "Settings…".into(),
                icon_name: "configure".into(),
                activate: Box::new(|_: &mut Self| open_settings()),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "Quit".into(),
                icon_name: "application-exit".into(),
                activate: Box::new(|t: &mut Self| {
                    info!("Quit requested from the tray");
                    let _ = t.shutdown_tx.send(true);
                }),
                ..Default::default()
            }
            .into(),
        ]);
        items
    }
}

fn open_release_page() {
    match std::process::Command::new("xdg-open")
        .arg(crate::update::RELEASES_PAGE)
        .spawn()
    {
        Ok(_) => info!("Opened the release page"),
        Err(e) => warn!("Could not open {}: {}", crate::update::RELEASES_PAGE, e),
    }
}

/// Best effort: a missing GUI binary is worth a log line, not a crash in the
/// middle of a menu callback.
fn open_settings() {
    match std::process::Command::new("uscreen-gui").spawn() {
        Ok(_) => info!("Opened settings from the tray"),
        Err(e) => warn!("Could not launch uscreen-gui: {}", e),
    }
}

/// Publish the tray icon and keep it in step with the daemon.
///
/// Returns without an icon if no StatusNotifierWatcher answers — a desktop
/// with no tray is a perfectly ordinary thing to run on.
pub async fn run(
    mode_tx: watch::Sender<bool>,
    mut tablet_rx: watch::Receiver<bool>,
    shutdown_tx: watch::Sender<bool>,
    mut update_rx: watch::Receiver<crate::update::Available>,
) {
    let mut mode_rx = mode_tx.subscribe();

    let tray = UScreenTray {
        pen_only: *mode_rx.borrow_and_update(),
        tablet_present: *tablet_rx.borrow_and_update(),
        update: update_rx.borrow_and_update().clone(),
        mode_tx,
        shutdown_tx,
    };

    let handle: Handle<UScreenTray> = match tray.spawn().await {
        Ok(h) => {
            info!("Tray icon registered");
            h
        }
        Err(e) => {
            warn!("No tray icon: {}. The daemon runs the same without one.", e);
            return;
        }
    };

    // Follow both signals for the lifetime of the daemon. The menu is rebuilt
    // from this state on every open, so an update here is all it takes for the
    // icon, the tooltip and the checkmark to agree with reality.
    loop {
        tokio::select! {
            r = mode_rx.changed() => {
                if r.is_err() { break; }
                let pen_only = *mode_rx.borrow();
                handle.update(move |t: &mut UScreenTray| t.pen_only = pen_only).await;
            }
            r = tablet_rx.changed() => {
                if r.is_err() { break; }
                let present = *tablet_rx.borrow();
                handle.update(move |t: &mut UScreenTray| t.tablet_present = present).await;
            }
            r = update_rx.changed() => {
                if r.is_err() { break; }
                let v = update_rx.borrow().clone();
                handle.update(move |t: &mut UScreenTray| t.update = v).await;
            }
        }
    }
}
