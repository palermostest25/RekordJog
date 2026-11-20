# RekordJog

**Use a Numark Mixtrack Go with rekordbox. One app, one installer, no setup files.**

rekordbox doesn't support the Mixtrack Go, and it only enables jog wheels for Pioneer hardware.
RekordJog sits in the middle. It shows rekordbox a virtual **Pioneer DDJ-SX** and translates every
Mixtrack Go control into the exact message a real DDJ-SX would send. rekordbox then uses its own
built-in DDJ-SX support, so jog wheels, pads, loops, effects, the mixer, the browser and the LEDs
all work. You don't import or replace any mapping files.

```
Mixtrack Go  ──USB──▶  RekordJog  ──"PIONEER DDJ-SX"──▶  rekordbox
     ▲                    │
     └────── LEDs ◀───────┘
```

- **Native app for macOS and Windows.** It's written in Rust, so there's no Python to install, no
  terminal, and nothing to configure.
- **Guided setup** that checks your controller, the virtual device and rekordbox, and tells you how
  to fix anything that's wrong.
- **Auto-connect.** It runs in the menu bar or system tray and reconnects whenever you plug the
  controller in. It can also start at login.
- **Remappable.** Every button and knob can be reassigned from a list, and changes apply instantly.
- **Built-in MIDI monitor** and a "Copy diagnostics" button for bug reports.

## Install

Download the latest installer from the [Releases page](../../releases). You can also get it from the
**Actions** tab: open the latest *Build installers* run and download its artifacts.

| | |
|---|---|
| **macOS** (Apple Silicon and Intel, 10.15+) | Open `RekordJog_x.y.z_universal.dmg` and drag RekordJog to Applications. The build isn't notarised yet, so the first time you open it, right-click the app and choose **Open**, or go to System Settings → Privacy & Security → **Open Anyway**. |
| **Windows** 10/11 | Run `RekordJog_x.y.z_x64-setup.exe`. It installs for your user only, with no admin prompt. If SmartScreen appears, click **More info → Run anyway**. |

**One-time extra step on Windows:** Windows has no built-in way for an app to create a MIDI device.
RekordJog uses the free virtualMIDI driver that comes with
[loopMIDI](https://www.tobias-erichsen.de/software/loopmidi.html). Install loopMIDI once and you're
done. You never need to open it or create ports, because RekordJog creates the "PIONEER DDJ-SX" port
itself. The setup assistant offers the download if the driver is missing and picks it up as soon as
it's installed. macOS needs nothing extra.

## First run

The setup assistant walks you through these steps:

1. **Connect.** Plug in the Mixtrack Go with a USB-C cable. Bluetooth is only for phones and tablets.
2. **Test.** Touch the jogs, move the faders and hit the pads. Each control lights up on screen.
3. **rekordbox.** RekordJog finds your rekordbox install and checks that its DDJ-SX mapping is intact.
   Open rekordbox in **PERFORMANCE** mode and choose the Mixtrack Go as the audio device
   (Preferences → Audio). The assistant confirms when rekordbox is connected.
4. **Tempo.** A quick check that the tempo fader moves the tempo the right way. One click flips it.

> rekordbox only enables controllers such as the DDJ-SX if your rekordbox plan includes DJ hardware
> (Core or higher).

## Default mapping

SHIFT = hold **MODE**. The controller handles SHIFT itself.

| Control | Does | With SHIFT |
|---|---|---|
| Jog wheel (top, touched) | Scratch (vinyl mode) / pitch bend | Fast search |
| Jog wheel (edge) | Pitch bend (nudge) | Fast search |
| Tempo fader | Tempo, 14-bit | – |
| Play | Play / Pause | Stutter |
| Cue | Cue | Jump to track start |
| Sync | Beat Sync | Set as tempo master |
| Acapel | 4-beat auto loop | Loop ÷2 |
| Instru | Beat FX on/off (left = FX1, right = FX2) | Loop ×2 |
| Headphone cue | Headphone cue | Tap BPM |
| MODE (tap) | Pad mode: Hot Cue → Beat Jump → Pad FX → Sampler → Pad FX 2 | – |
| Pads 1–4 | Follow the pad mode, with LEDs | Shift functions (for example, delete hot cue) |
| Level knob | Channel fader | – |
| Filter/Low knob | Color FX filter (or Low EQ) | – |
| Crossfader | Crossfader | – |
| Fade FX | Fade FX: the crossfader also sweeps a filter open on the deck you fade out | Library back |
| Browse (turn / push) | Browse / open folder | Waveform zoom |
| Load left / right | Load to deck 1 / 2 | Switch Filter ↔ Low EQ / toggle vinyl mode |
| Main / Cue level | Not mapped (assign in **Mapping**) | – |

You can change anything under **Mapping**. The jog resolution is calibrated so one turn of the
platter equals one turn of the record on screen. You can adjust scratch and nudge sensitivity under
**Settings**.

## Troubleshooting

The **Status** page diagnoses most problems. The common ones:

- **"Mixtrack Go found but can't be opened"**: another app (Serato, djay, VirtualDJ, Mixxx, or
  rekordbox on Windows) is using it. Close that app, or quit rekordbox, wait for RekordJog to
  connect, then reopen rekordbox.
- **"Restart rekordbox"**: rekordbox was opened before the virtual DDJ-SX existed. Quit and reopen
  rekordbox. RekordJog can stay running.
- **"rekordbox's DDJ-SX file was replaced by the old RekordJog"**: RekordJog 1.x asked you to
  overwrite `PIONEER DDJ-SX.midi.csv` in rekordbox's folder. Reinstall rekordbox to restore the
  original.
- **Tempo goes the wrong way**: Settings → *Reverse tempo fader direction*.
- **Something else**: open **MIDI monitor**, turn on *Record*, reproduce the problem, then click
  **Copy diagnostics** and paste the result into an issue.

## How it works

| Part | What it does |
|---|---|
| `core/` | Pure Rust translation engine with no I/O and full unit tests. Mixtrack Go → DDJ-SX messages, LED feedback from DDJ-SX → Mixtrack Go, pad modes, 242→2048-tick jog scaling, 14-bit faders, Fade FX, remapping. |
| `app/` | Tauri 2 desktop app. Handles MIDI hot-plug, creates the virtual port (CoreMIDI on macOS, the virtualMIDI driver on Windows, loopMIDI port as a fallback), detects rekordbox and runs the diagnostics. |
| `ui/` | Plain HTML/CSS/JS for the interface, with no build step. |
| `docs/` | The stock rekordbox `PIONEER DDJ-SX.midi.csv`, used as the reference for every message. |

The Mixtrack Go's MIDI messages come from the community
[Mixxx mapping](https://github.com/stefets/Numark-Mixtrack-Go-mapping-for-Mixxx). Thanks to its
authors.

## Build from source

You need [Rust](https://rustup.rs) and Node.js (Node is only used to fetch the Tauri CLI).

```bash
cargo test -p rekordjog-core
```

```bash
cd app && npx @tauri-apps/cli@2 dev
```

```bash
cd app && npx @tauri-apps/cli@2 build
```

On Linux you also need the usual Tauri packages: `libwebkit2gtk-4.1-dev`, `libasound2-dev`,
`libayatana-appindicator3-dev`. The app builds and runs there, but rekordbox doesn't.

Pushing a `v*` tag builds the macOS `.dmg` and Windows `-setup.exe` in GitHub Actions and attaches
them to a release. To sign and notarise the macOS app, add the standard Tauri `APPLE_*` secrets.

### Supporting another controller

Controller definitions live in `core/src/mixtrack.rs`, and every target message lives in
`core/src/ddj.rs`. A new controller needs its own control table and jog resolution. Pull requests are
welcome.

## Older versions

The original Python scripts for the Xone:4D, Hercules DJControl and Mixtrack 3 are in
[`legacy/`](legacy/NOTE.md).

## License

MIT. See [LICENSE](LICENSE).
