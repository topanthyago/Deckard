# Validation

Validation commands run on Linux with Rust/Cargo 1.95.0 and Nix-provided ALSA,
libudev, libusb, CMake and pkg-config. Builds use two jobs at lowered priority.
The repository's recommended toolchain remains the version in `mise.toml`.

## Completed checks

- `cargo metadata --locked --no-deps` resolves the Deckard and bridge workspaces.
- `cargo test --locked -p deckard-surface` passes 258 tests, with one existing
  ignored test. It includes shipped MK2 discovery, real-host startup and the
  Lua controller regression harness.
- The standalone bridge workspace passes 222 daemon tests and three ALSA-wrapper
  tests. One explicit host-sequencer roundtrip is ignored by default.
- The launcher's two Python unit tests pass.
- The final serial viewer run passes 641 tests, with 16 existing ignored tests
  and four baseline cases filtered out as detailed below.
- Formatting checks pass for the Deckard workspace, bridge workspace and the
  four patched native eframe files.
- Clippy passes for the bridge workspace, scoped viewer/surface targets and
  the vendored eframe library, with style and unused-code warnings.
- Gitleaks scans the checkout with no findings.

The historical bridge source produces style and unused-code warnings. The
vendored bridge's public raw-event conversion now carries an explicit unsafe
contract and a `Safety` section, resolving its default-deny Clippy diagnostic.
Upstream copyright and license notices are retained.

## Existing viewer diagnostics

The full viewer test binary was built through `cargo test --locked -p deckard`.
An unmodified worktree at upstream commit
`6bb10ff50b8d63c23237042889b1155342a0d3c4` was built with the same toolchain and
used for isolated comparisons.

These cases reproduce on that upstream baseline in the current environment.

- `midi_io::tests::clock_in_feeds_the_follower` fails its received-data check.
- `midi_io::tests::keyboard_to_sound_to_synth` fails its received-data check.
- `midi_io::tests::timed_messages_wait_until_due` fails its received-data check.
- `rec_tests::generate_keeps_to_the_scale_picked_in_the_software` exceeds a
  20-second timeout. The publication branch's broad run was terminated after
  this case continued beyond two minutes.

Other parallel-run UI/audio failures passed on isolated retries. The final
serial viewer run excludes the four baseline cases listed above.

Clippy with dependency linting reaches the existing `approx_constant` error in
`crates/model/src/lfo.rs`, line 363 (`0.70711`). Scoped linting uses
`cargo clippy --locked --no-deps -p deckard -p deckard-surface --all-targets`.
The bridge uses its separate workspace Clippy command.

The serial viewer invocation is reproducible with

```sh
cargo test --locked -p deckard -- --test-threads=1 \
  --skip rec_tests::generate_keeps_to_the_scale_picked_in_the_software \
  --skip midi_io::tests::clock_in_feeds_the_follower \
  --skip midi_io::tests::keyboard_to_sound_to_synth \
  --skip midi_io::tests::timed_messages_wait_until_due
```


## Physical checks

The originating Linux integration confirmed pad RGB, display data, the TUNE
encoder, browser navigation/loading, MUTE and PLAY on a full-size MK2. This
publication's automated checks use synthetic controller/project state and
isolated ALSA test ports. Compatibility on additional Linux systems awaits
independent testing.
