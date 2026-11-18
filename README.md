# acousticmultimixer

A GStreamer multi mixer with acoustic space modelling, written in Rust.

Every input is one person's voice; every output is what one person hears: everyone else (mix-minus), placed in a
shared virtual space by distance and direction, with early reflections and a shared reverb from a room preset. It is
being built for [SpatialAudioServer](https://github.com/JakDaniels/SpatialAudioServer), an audio conferencing server
(MCU) with spatial audio, and is meant to be useful to any GStreamer application.

**Status:** under development; nothing usable yet. The plan and the decisions: [PLAN.md](PLAN.md); the original
design: [docs/design.md](docs/design.md); what changed: [CHANGELOG.md](CHANGELOG.md).

## Building

Needs Rust 1.92 or newer (through [rustup](https://rustup.rs)) and GStreamer 1.24 or newer with its development
files (Ubuntu: `libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev`).

```bash
cargo build                                                    # target/debug/libgstacousticmultimixer.so
GST_PLUGIN_PATH=$PWD/target/debug gst-inspect-1.0 acousticmultimixer
cargo test
```

## Licence

Copyright 2026 Simon Annetts/Jak Daniels. [MPL-2.0](LICENSE), as most of gst-plugins-rs.
