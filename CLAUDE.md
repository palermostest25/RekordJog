# RekordJog – notes for Claude

RekordJog makes a Numark Mixtrack Go work in rekordbox by presenting a virtual
"PIONEER DDJ-SX". Rust workspace: `core/` (pure translation engine, unit tested),
`app/` (Tauri 2 desktop app: MIDI I/O, virtual port, rekordbox detection, diagnostics),
`ui/` (plain HTML/CSS/JS, no build step). See README.md for the full picture.

## Always publish

The owner wants every change shipped as a GitHub release — never leave work
unreleased, and don't ask first.

- After any change: run `cargo test --workspace`, bump the **patch** version in
  both `Cargo.toml` (`[workspace.package] version`) and `app/tauri.conf.json`
  (`version`) — they must match — then commit and push.
- Pushing triggers `.github/workflows/build.yml`, which builds the macOS `.dmg`
  and Windows `-setup.exe` and publishes/updates the release `v<version>`
  (it creates the tag on GitHub). Cloud sessions can't push tags (HTTP 403),
  so never rely on pushing a tag yourself.
- The app's auto-updater (`app/src/updater.rs`) reads `releases/latest` from
  `palermostest25/RekordJog` and compares versions, so an unbumped version never
  reaches users.
- Then confirm it worked: check the Actions run succeeded and the release has
  both installers attached, and give the user the release link.

## Conventions

- Mixtrack Go MIDI codes: `core/src/mixtrack.rs` (from the community Mixxx mapping).
- DDJ-SX messages and the remappable action catalog: `core/src/ddj.rs`; reference
  file `docs/PIONEER DDJ-SX.midi.original.csv` (stock rekordbox mapping).
- Add a unit test in `core/src/engine.rs` for any translation change.
- `cargo check -p rekordjog --target x86_64-pc-windows-msvc` works on Linux;
  macOS can only be verified in CI.
