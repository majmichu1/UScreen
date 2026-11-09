# Configuration

Everything is in `~/.config/uscreen/config.toml`. The file is created on first
run; every key is optional and a missing key keeps its default. Values outside
the allowed range are clamped to the nearest allowed one (the daemon logs a
warning), and an unknown `position` falls back to `"right"`. Most of this can
also be changed from `uscreen-gui`, the ⚙ sheet in the tablet app, the tray
icon or CLI flags (flags win over the file). Restart the daemon, or press
**Apply**, for a change to take effect.

| Key | Default | Allowed | What it does |
|---|---|---|---|
| `encoder` | `h264_nvenc` | `h264_nvenc`, `hevc_nvenc`, `h264_vaapi`, `libx264` | Video encoder. `uscreen doctor` shows which ones work on this machine; `uscreen check-encoder -- NAME` tests one without starting the daemon. |
| `fps` | `60` | 10–90 | Target frame rate. |
| `bitrate` | `20000` | 1000–60000 (kbps) | Ceiling for bursts. With constant quality it is `quality`, not this, that sets sharpness. |
| `quality` | `18` | 12–32 | Constant-quality target: lower is sharper and uses more bits. |
| `width`, `height` | `2960`, `1848` | 640–8192, 480–8192 | Size of the virtual display. Only used when `auto_resolution = false`. |
| `auto_resolution` | `true` | | Make the virtual display follow the resolution the tablet reports on every connect. Set `false` to keep your own `width` and `height`. |
| `stream_scale` | `1` | 1–4 | Integer downscale of the stream only; the desktop keeps its mode. `2` sends a quarter of the pixels: lower decode time, softer text. |
| `ten_bit` | `false` | | HEVC Main10. More precision against banding, not more colour; not HDR. |
| `position` | `"right"` | `right`, `left`, `above`, `below` | Where the virtual screen sits relative to your real ones. |
| `pen_only` | `false` | | Graphics-tablet mode: nothing is streamed, the pen and touch drive your own screen. Also switchable from the tablet. |
| `pointer_handoff` | `true` | | Leave an ordinary cursor where the pen was lifted. `false` creates no such device; the mouse stays where it was. |
| `max_tablets` | `1` | 1–4 | How many tablets at once, each its own screen. Needs that many EVDI devices (the installer sets them). |
| `vaapi_device` | `""` | a render node | e.g. `/dev/dri/renderD129`. Empty picks the first node that really encodes a frame. |
| `wifi_address` | `""` | `ip:port` | Set by `uscreen wifi`; the daemon reconnects to it when the cable is out. Empty disables it. |
| `adb_transport` | `"auto"` | `auto`, `usb`, `wifi` | Which adb connection to drive the tablet over. `auto` prefers USB and uses Wi-Fi when the cable is out; `usb` ignores Wi-Fi entirely (no reconnects either); `wifi` ignores USB. A tablet reachable both ways is one tablet in every mode. |
| `auto_launch_app` | `true` | | Start the app on the tablet when it is plugged in. |
| `require_token` | `true` | | The tablet must present this run's session token before it gets video or can inject input. Turn off only for an app older than 1.1.0. |
| `check_updates` | `true` | | One HTTPS request a day to api.github.com to tell you a newer release exists. Nothing is downloaded or installed. |
| `video_port`, `input_port` | `8890`, `8891` | | Loopback ports for video and for input/control. Change them only if something else uses these. |

## Per-tablet settings

Each tablet is remembered by its adb serial in a `[tablets.<serial>]` block,
written when it first reports its panel:

```toml
[tablets.R52W10ABCDE]
label = "Galaxy Tab S9 Ultra"
width = 2960
height = 1848
width_mm = 326
height_mm = 209
# optional, only where this tablet should differ from the global values:
fps = 60
bitrate = 20000
quality = 18
stream_scale = 1
position = "right"
```

Size and millimetres let the right display (and the right desktop scale) be
built the moment the tablet is plugged in, instead of after it has answered.

## Security notes

The ports listen on `127.0.0.1` only; the tablet reaches them through
`adb reverse`. Leave `require_token` on unless you have to.
