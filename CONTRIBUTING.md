# Contributing

The most useful contribution right now is a **compatibility report**: which
distribution, desktop, GPU, tablet and Android version you ran this on, and
whether it worked. There is an issue template for it, and `uscreen doctor`
prints most of the details you need. Every report becomes a row in
[docs/compatibility.md](docs/compatibility.md).

## Bugs

Open an issue with the output of `uscreen doctor` and, if the daemon is
involved, the log (`RUST_LOG=uscreen=debug uscreen start`, or
`journalctl --user -u uscreen`). Say what you expected to happen.

## Code

- Build with `make build` (Rust host + C helper) and `cd android && ./gradlew
  assembleDebug` for the app. See [docs/development.md](docs/development.md).
- Run `cargo test --release --manifest-path host/Cargo.toml` and `cargo clippy`
  before opening a pull request; both are expected to be clean.
- Keep commits focused and write the message for someone reading `git log`
  in a year: what broke, why, what changed.
- Measure before claiming a performance change. The daemon logs end-to-end
  latency percentiles; quote them.

## Pull requests

One change per PR. Describe how you tested it and on what hardware. If it
touches the protocol between app and daemon, update both and say so — they
ship together.

## Good first issues

Issues tagged `good first issue` are self-contained and come with pointers to
the relevant code. Ask in the issue if anything is unclear.
