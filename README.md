# acousticmultimixer

A GStreamer multi mixer with acoustic space modelling, written in Rust.

Every input is one person's voice; every output is what one person hears: everyone else (mix-minus), placed in a
shared virtual space by distance and direction, with early reflections and a shared reverb from a room preset. It is
being built for [SpatialAudioServer](https://github.com/JakDaniels/SpatialAudioServer), an audio conferencing server
(MCU) with spatial audio, and is meant to be useful to any GStreamer application.

**Status:** under development. For now it is a plain mix-minus mixer (every output hears all the other inputs);
the acoustics come next. The plan and the decisions: [PLAN.md](PLAN.md); the original
design: [docs/design.md](docs/design.md); what changed: [CHANGELOG.md](CHANGELOG.md).

## Building

Needs Rust 1.92 or newer (through [rustup](https://rustup.rs)) and GStreamer 1.24 or newer with its development
files (Ubuntu: `libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev`).

```bash
cargo build                                                    # target/debug/libgstacousticmultimixer.so
GST_PLUGIN_PATH=$PWD/target/debug gst-inspect-1.0 acousticmultimixer
cargo test
```

## Example

```bash
gst-launch-1.0 acousticmultimixer name=m \
  audiotestsrc freq=440 ! m.sink_0 \
  audiotestsrc wave=ticks ! m.sink_1 \
  audiotestsrc freq=220 ! m.sink_2 \
  m.mix_0 ! audioconvert ! autoaudiosink      # ticks and 220 Hz: everyone but sink_0
```

Inputs are `sink_0` to `sink_31` (`max-participants`, default 32); each has its own output `mix_N`. When more than
one output is linked, put a `queue` after each.

## Licence

Copyright 2026 Simon Annetts/Jak Daniels. [MPL-2.0](LICENSE), as most of gst-plugins-rs.

The element's structure (a bin holding an AudioAggregator-based mixer and a splitter) and its first tests come from
the `minus1mixer` element in gst-plugins-rs's `audiomultimixer` plugin, by Tim-Philipp Müller, Taruntej
Kanakamalla and Sebastian Dröge (Centricular), also MPL-2.0; the files taken from it keep their copyright lines.
