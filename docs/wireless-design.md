# Wireless without adb: design

Status: **draft, nothing is built.** This is the write-up promised in
[#8](https://github.com/majmichu1/UScreen/issues/8). Comments are welcome there
before any code is written.

## Goal

The tablet finds the computer on the local network and connects to it with no
adb and no cable, and nobody else on that network can watch the screen or
inject input.

Not a goal: replacing USB. USB stays the default and the lowest-latency path.
Nothing listens on the network unless the user turns wireless on.

## What exists today

- Video is a TCP stream of length-prefixed Annex B frames on port 8890, input
  and control is a WebSocket on 8891. Both listen on `127.0.0.1` only.
- The tablet reaches them through `adb reverse`. A per-session token, created
  by the daemon, is handed to the app as an intent extra when the daemon starts
  it, and the app must present it first or the socket is closed.
- Wi-Fi today is adb's: `uscreen wifi` keeps adb connected over the network and
  everything above still runs through `adb reverse`.
- Nothing is encrypted. That is acceptable on loopback and over USB, and not
  acceptable on a network.

## Threat model

On the same network an attacker could:

1. connect to an open port and watch the screen or send clicks and keystrokes;
2. sniff the stream on shared Wi-Fi (screen content, pen strokes);
3. pretend to be the computer, so the tablet sends its touches to the wrong
   machine;
4. sit in the middle of the pairing step;
5. guess a pairing code.

The design has to answer all five. Not covered: malware already running as the
user on the computer or the tablet.

## Design

**Pairing, once per tablet.** The computer shows a one-time code (and a QR code
carrying the same secret and the computer's key fingerprint) in
`uscreen pair`, the GUI and the tray. The tablet scans the QR code, or the
user types the six digits. The code is used inside a password-authenticated
key exchange (SPAKE2 or similar), so a six-digit code cannot be brute-forced
offline from a captured exchange. It is valid for two minutes, single use,
three attempts. At the end both sides hold each other's long-term public key.

**Trust store.** The computer keeps a list of paired tablets (id, public key,
label, date) in a file next to `config.toml`. The tablet keeps the computer's
key in the Android Keystore. `uscreen pair list` and `uscreen pair revoke`
manage it; a lost tablet is revoked on the computer.

**Transport.** TLS 1.3 with mutual authentication, using the pinned keys from
pairing instead of certificate authorities. Chosen over a custom protocol
(Noise and the like) because both ends already have audited TLS stacks and I
would rather not ship my own crypto. The existing message formats stay as they
are, wrapped in TLS; the client certificate replaces the session token as
proof of identity. A separate listener binds to the LAN interface only when
`wireless = true`; the loopback ports and the token path for USB do not change.

**Discovery.** mDNS / DNS-SD, service `_uscreen._tcp`, with the host name, port,
protocol version and a short key fingerprint in the TXT record. Android has
`NsdManager` for it. Many access points block mDNS between clients, so manual
entry of the address (and the QR code, which carries it) always works, and so
does an address on a VPN such as Tailscale.

**Video over Wi-Fi.** Starts as TCP, as today, because the first goal is
security and discovery, not latency. The measured problem with Wi-Fi is the
tail ([benchmarks](benchmarks.md#usb-vs-wi-fi-h264-quiet-link)): one lost
packet stalls everything behind it. A loss-tolerant transport (UDP or QUIC,
dropping to the next keyframe on loss) is the real fix, but I want numbers from
the TCP version before choosing, so it is a later phase.

## Phases

1. **TLS listener, pairing with QR code or typed code, manual address.** The
   smallest thing worth shipping. No mDNS yet.
2. **mDNS discovery** and a list of computers in the app.
3. **Loss-tolerant video transport**, only after measuring phase 1 on real
   Wi-Fi.
4. Pairing and device management in the GUI and tray.

The wire protocol gets a version number in the first message, so an app and a
daemon of different ages say so instead of failing oddly. USB keeps working
unchanged for old apps.

## Open questions

- QR code first and the typed code as a fallback, or the other way round? The
  QR code is stronger and needs the camera permission.
- Firewalls: the LAN port has to be opened (`ufw`, `firewalld`). The installer
  should not do that silently; `uscreen doctor` should say what is blocking.
- IPv6 and several computers on one network.
- Which Android version to require for the pairing flow (Keystore, `NsdManager`,
  camera).
- A second pair of eyes on the pairing and TLS part before it ships. I would
  like a review by someone who does this for a living.

I have not prototyped any of this yet. The Android side (custom trust
manager with pinned keys, `NsdManager` behaviour across vendors) is what I
would try first.
