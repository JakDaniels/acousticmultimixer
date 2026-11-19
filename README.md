# acousticmultimixer

A GStreamer multi mixer with acoustic space modelling, written in Rust.

Every input is one person's voice; every output is what one person hears: everyone else (mix-minus), placed in a
shared virtual space by distance and direction, with early reflections and a late reverb from a room preset. It is
being built for [SpatialAudioServer](https://github.com/JakDaniels/SpatialAudioServer), an audio conferencing server
(MCU) with spatial audio, and is meant to be useful to any GStreamer application.

**Status:** under development. Working now: mix-minus in stereo, with level by distance (SpatialAudioServer's
formulas), direction by a simple head model (the far ear hears later, quieter and duller) or a stereo pan, air
absorption, per-person gain, mute and trims, and rooms (`room`) with early reflections and late reverb.
The plan and the decisions: [PLAN.md](PLAN.md); the original design: [docs/design.md](docs/design.md); what
changed: [CHANGELOG.md](CHANGELOG.md).

## Installing

Packages are on the [Releases page](https://github.com/JakDaniels/acousticmultimixer/releases), each with its
`SHA256SUMS`:

- **Debian 13 or later, Ubuntu 24.04 or later** (x64 or arm64): `gstreamer1.0-acousticmultimixer`, e.g.
  `sudo apt install ./gstreamer1.0-acousticmultimixer_0.1.0_amd64.deb`.
- **Fedora** (x64 or arm64): `gstreamer1-acousticmultimixer`, e.g.
  `sudo dnf install ./gstreamer1-acousticmultimixer-0.1.0-1.x86_64.rpm`.
- **Windows** (GStreamer 1.28, MSVC x64): the zip holds `gstacousticmultimixer.dll`; put it in GStreamer's
  `lib\gstreamer-1.0` folder, or in a folder named by `GST_PLUGIN_PATH`.

SpatialAudioServer's apt and dnf repositories (https://jakdaniels.github.io/SpatialAudioServer/) carry the Linux
packages too, and its Windows installers include the DLL.

## Building

Needs Rust 1.92 or newer (through [rustup](https://rustup.rs)) and GStreamer 1.24 or newer with its development
files (Ubuntu: `libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev`).

```bash
cargo build                                                    # target/debug/libgstacousticmultimixer.so
GST_PLUGIN_PATH=$PWD/target/debug gst-inspect-1.0 acousticmultimixer
cargo test
```

The packages: `scripts/build-packages.sh` (needs [nfpm](https://nfpm.goreleaser.com); the version comes from the git
tags). Releases are made by merging a pull request from `develop` into `main` (`.github/workflows/release.yml`).

## Example

```bash
gst-launch-1.0 acousticmultimixer name=m sink_1::y=4 sink_2::x=8 \
  audiotestsrc freq=440 ! m.sink_0 \
  audiotestsrc wave=ticks ! m.sink_1 \
  audiotestsrc freq=220 ! m.sink_2 \
  m.mix_0 ! audioconvert ! autoaudiosink
```

What participant 0 hears, at the origin facing east (+x): the ticks 4 m to the left, the 220 Hz tone 8 m ahead.
Inputs are `sink_0` to `sink_31` (`max-participants`, default 32), each with its own stereo output `mix_N`; with
more than one output linked, put a `queue` after each. `gst-inspect-1.0 acousticmultimixer` lists the
element's properties; the pad properties (`x`, `y`, `z`, `yaw`, `pitch`, `roll`, `listener-*`, `gain`, `mute`,
`trims`) are described in [PLAN.md](PLAN.md#3-the-elements-interface).

## Licence

Copyright 2026 Simon Annetts/Jak Daniels. [MPL-2.0](LICENSE), as most of gst-plugins-rs.

The element's structure (a bin holding an AudioAggregator-based mixer and a splitter) and its first tests come from
the `minus1mixer` element in gst-plugins-rs's `audiomultimixer` plugin, by Tim-Philipp Müller, Taruntej
Kanakamalla and Sebastian Dröge (Centricular), also MPL-2.0; the files taken from it keep their copyright lines.
