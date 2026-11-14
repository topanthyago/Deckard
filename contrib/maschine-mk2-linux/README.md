# Maschine MK2 on Linux

This branch adds the full-size Native Instruments **Maschine Controller MK2**
(USB `17cc:1140`) to Deckard 0.5.0. A user-space HID bridge exposes ALSA MIDI
ports. Deckard ships the matching Lua controller script and connects when the
bridge's `Controls MIDI` ports appear.

The controller provides RGB pads, both group emitters, two monochrome displays,
transport, parameter encoders, browser navigation/loading, mute/solo/select,
pattern and scene selection, step sequencing and sampling controls. LCD and LED
updates are differential. Display frames use checksums and acknowledgements.
The patched eframe event loop keeps MIDI processing active on hidden workspaces.

## Build

Use Rust 1.99.1 as pinned in the repository's `mise.toml`, a C/C++ toolchain,
CMake and pkg-config. Linux development headers include ALSA, libudev and
libusb. For example, on Debian/Ubuntu install the build dependencies with

```sh
sudo apt install build-essential cmake pkg-config libasound2-dev libudev-dev libusb-1.0-0-dev
```

Clone the integration branch and build both programs

```sh
git clone --branch codex/maschine-mk2-linux https://github.com/topanthyago/Deckard.git
cd Deckard
cargo build --release --locked -p deckard
cargo build --release --locked --manifest-path contrib/maschine-mk2-linux/daemon/Cargo.toml
```

The vendored eframe patch is selected by Cargo automatically. All bridge source,
licenses and its dependency lock are included. The normal Deckard build and
Linux runtime requirements are described in [DEVELOPMENT.md](../../DEVELOPMENT.md).

## USB access and launch

Install the udev rule, reload rules, then unplug and reconnect the MK2

```sh
sudo install -m644 scripts/linux/50-deckard-maschine-mk2.rules /etc/udev/rules.d/
sudo udevadm control --reload-rules
```

Run the bridge in a terminal as your logged-in desktop user

```sh
python3 contrib/maschine-mk2-linux/launch.py "$PWD/contrib/maschine-mk2-linux/daemon/target/release/maschine"
```

The launcher waits for the MK2's accessible `*-if02-hidraw` by-id link and
recovers after unplugging it. Its working directory is
`${XDG_DATA_HOME:-$HOME/.local/share}/deckard/maschine-mk2`. An optional
`maschine.json` there holds persistent configuration. With defaults, brightness
and contrast retain their hardware values. The bridge listens for OSC on
`127.0.0.1:42434` and sends OSC updates to `127.0.0.1:42435`.

In another terminal, start Deckard

```sh
./target/release/deckard
```

In **Preferences → MIDI**, enable `Pads MIDI` for notes and set the
**Maschine MK2 drum channel** to **1**. `Controls MIDI` is reserved automatically for the controller
script. MK2 discovery uses these MIDI ports. The **Controller Model** setting
selects the model for the MK3/NI script layer. In a new project, add an instrument through **Sound → + → Internal**.
The controller's first pad is bottom-left, sending note 36 on channel 1.

### Optional user service

Install the launcher and bridge into your user directories, then enable the
provided systemd user service

```sh
install -Dm755 contrib/maschine-mk2-linux/daemon/target/release/maschine "$HOME/.local/bin/maschine"
install -Dm644 contrib/maschine-mk2-linux/launch.py "$HOME/.local/share/deckard/maschine-mk2/launch.py"
install -Dm644 contrib/maschine-mk2-linux/maschine-mk2.service "$HOME/.config/systemd/user/maschine-mk2.service"
systemctl --user daemon-reload
systemctl --user enable --now maschine-mk2.service
```

The example service uses `/usr/bin/python3`. Adjust `ExecStart` for systems
with another Python installation. Run one bridge instance for the controller.

## Controls

- Pads play sounds. A–H selects a group. Hold MUTE, SOLO or SELECT while pressing
  a pad/group to modify that selection. RGB colors follow the project.
- PLAY, REC and RESTART control transport. The eight knobs edit the current
  parameter page. SHIFT makes encoder changes finer.
- BROWSE opens the library. The large encoder and navigation arrows select
  results. ENTER or button 8 loads the selection. SHIFT + 8 previews it.
  Buttons 1–4 and knobs 1, 2, 5–8 adjust filters. Button 7 toggles favorites.
- PATTERN toggles the step sequencer. Pads toggle steps. SELECT + pad selects
  a sound. STEP or button 5 toggles FOLLOW. Page arrows and buttons 7–8 change
  page and disable FOLLOW. Knob 1 changes pattern length and knob 2 the grid.
  SHIFT + PATTERN opens pattern selection. SCENE selects scenes by pad.
- SAMPLING opens the sound editor. Buttons 1–4 select RECORD, EDIT, SLICE and
  ZONE. Knobs 1–4 edit parameters, with page arrows for additional parameters.
  Knobs 5–6 zoom/scroll waveforms. Buttons 5–8 expose the current tab's actions.
  ENTER previews and SHIFT + ENTER shows the full sample.

The display follows Deckard's state and shows parameters, browser results,
patterns, scenes and sampling waveforms. Long labels are clipped to the cells.
Some original Maschine combinations, including ALL and NAV, have no mapping.
ZONE creation uses **SLICE → APPLY**. Stem separation uses Deckard's configured
model and resources.

## Checks

```sh
cargo test --locked -p deckard-surface
cargo test --locked --manifest-path contrib/maschine-mk2-linux/daemon/Cargo.toml --workspace
python3 -m unittest discover -s contrib/maschine-mk2-linux -p 'test_*.py'
```

The surface tests exercise shipped-device discovery, the real Lua host and
scripted controller regressions. The bridge tests cover parser bounds,
RGB feedback, simultaneous button changes, display regions and atomic transport.
These checks use synthetic state. Physical checks on the originating Linux setup
confirmed pad RGB, display data, the TUNE encoder, browser navigation/loading,
MUTE and PLAY. Runtime compatibility on other Linux systems remains to be tested.

Viewer and lint results, including the upstream-baseline diagnostics, are
recorded in [VALIDATION.md](VALIDATION.md).

For diagnosis use `aconnect -l`, `systemctl --user status maschine-mk2` and
`journalctl --user -u maschine-mk2`. Check device permissions and the by-id link
when the launcher is waiting. The controller Lua source is shipped in
`crates/surface/lua/devices/maschine_mk2/` and supports Deckard's usual
controller-folder overrides.

[Source provenance and licenses](PROVENANCE.md) describe the standalone bridge
and its upstream lineage.
