# Deckard background controller support

Source is the published `eframe` 0.36.2 crate from crates.io, SHA-256
`55ad41fac6e3149abb4866e93df37dbd33baa95db525c3368c944584e95a5dc2`.
The crate's `.cargo_vcs_info.json` records the original egui revision.
The original MIT and Apache-2.0 license texts were retrieved from the
[egui 0.36.2 tag](https://github.com/emilk/egui/tree/0.36.2).
The normalized Cargo manifest's license include paths point at those local files.

Deckard patches the native event loop to run application logic when a compositor
withholds redraw callbacks for a hidden/offscreen window. A 25 ms pending-redraw
deadline enters logic-only operation. Real redraws restore painting. Both glow
and wgpu use the same signal. This lets the app process MIDI while hidden.

The workspace `[patch.crates-io]` makes ordinary locked Cargo builds use this
source. Global Cargo caches are unaffected. Local changes preserve eframe's
MIT OR Apache-2.0 license.
