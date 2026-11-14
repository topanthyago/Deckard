# Source and licenses

The standalone `daemon/` source comes from
[Witzman/Generative-Techno-ZynthianMaschine-MKII](https://github.com/Witzman/Generative-Techno-ZynthianMaschine-MKII/tree/7f5a24e1ed7a6c6e404137626bc7544410819990/daemon),
commit `7f5a24e1ed7a6c6e404137626bc7544410819990`.
Its GitHub API source tarball SHA-256 is
`2b5baca3808fa627a7605f71475ec9809192fc5c6989d052de3bcc48239c288d`.

The original root `LICENSE`, `daemon/LICENSE`, `daemon/CREDITS.md`, source
notices and `alsa-seq/LICENSE` are preserved. The bridge carries a GPL-3.0
license text and historical LGPL-3.0-or-later source headers. These upstream notices are retained. Its local ALSA wrapper
retains its MIT license.
The bridge runs as a separate process and exchanges MIDI with Deckard.
Deckard retains its existing GPL-2.0-or-later licensing.

Local changes add separate pad/control ALSA ports, Deckard's control mapping,
RGB pad and dual-emitter group feedback, differential LCD graphics, checked
atomic display frames, FIFO ACK handling, reconnect support and regression
checks. The vendored lock pins the bridge dependencies. The standalone bridge
has its own Cargo workspace. Upstream helper shell scripts were omitted.

The controller Lua files in `crates/surface/lua/devices/maschine_mk2/` and the
viewer changes use Deckard's existing license. `launch.py` and its Python tests
are distributed under GPL-3.0-or-later, see the accompanying `LICENSE` text.
The controller regression harness follows Deckard's license.
