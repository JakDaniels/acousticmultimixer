# acousticmultimixer: plan

A GStreamer element, written in Rust, that mixes N mono voices into one personal stereo mix per listener (mix-minus),
placing each voice in a shared virtual acoustic space: distance, direction (level and time differences between the
ears), early reflections and a shared late reverb, with room presets. Its first user is
[SpatialAudioServer](https://github.com/JakDaniels/SpatialAudioServer) (issue #8 there), as a new mix engine for
spatial rooms; the aim is for it to be generally useful and, in time, to be offered to gst-plugins-rs.

- The original design, with the DSP ideas and the cost analysis: [docs/design.md](docs/design.md). This plan
  holds the decisions; where they differ, this plan wins.
- Status: **planning**. Nothing is built yet.

## 1. Decisions so far

| | Decision | Since |
|---|---|---|
| Name | `acousticmultimixer` for the repository, the plugin, the crate (`gst-plugin-acousticmultimixer`) and the element | 2026-11-18 |
| Language | Rust, with gstreamer-rs (design.md, "Language: Rust") | 2026-11-16 |
| Licence | MPL-2.0, as gst-plugins-rs, so no relicensing if adopted | 2026-11-16 |
| GStreamer | runs on 1.24 or newer; Linux builds against 1.24, Windows (MSVC) against the 1.28 that SpatialAudioServer's MSI embeds | 2026-11-16 |
| Control | properties on the request pads (position, orientation, gain) and on the element (room, distance model), not custom events | 2026-11-16 |
| Template | gst-plugins-rs's `audiomultimixer` plugin (the `minus1mixer` element): its structure solves the "many outputs" problem (section 2) | 2026-11-18 |
| Toolchain | Rust stable through rustup (in `~/.cargo`), clippy, rustfmt, cargo-c | 2026-11-18 |
| Pair gains | the element computes level and direction from positions; the application may add a gain per (speaker, listener) pair on top (0 = muted), for per-person volume | 2026-11-18 |
| Speaker facing | the `yaw` of a voice is accepted from the start but has no effect until the geometry work (Later) | 2026-11-18 |
| Pad roles | one kind of participant: every `sink_N` has its `mix_N`; listen-only = `sink_N` never fed, talk-only = `mix_N` never linked (skipped) | 2026-11-18 |

## 2. The template: gst-plugins-rs `minus1mixer`

`audio/audiomultimixer` in gst-plugins-rs (Tim-Philipp Müller, Centricular; added to gst-plugins-rs main 2026-07-30,
"Since 0.16.0", **not yet in a release**; cloned at `docs/reference/gst-plugins-rs` in SpatialAudioServer) already
does the plain version of our job: for every input `sink_N` it gives an output `minus_N` with everyone else mixed.

**How it works.** `minus1mixer` is a `gst::Bin` holding two internal elements:

```text
            +------------------------------- minus1mixer (Bin) ------------------------------+
sink_0 ---> |  mixer (AudioAggregator subclass)            splitter (gst::Element)           | ---> minus_0
sink_1 ---> |  one output buffer holding every    ----->   one request src pad per output;  | ---> minus_1
sink_2 ---> |  output's channels side by side,             cuts each output's channels out   | ---> minus_2
            |  plus a SplitMeta saying which               and pushes them (GAP events for   |
            |  channels belong to which output             outputs with nothing this time)   |
            +--------------------------------------------------------------------------------+
```

- The **mixer** subclasses `GstAudioAggregator`, so it inherits what is hard to get right: syncing live inputs on
  the clock, timeouts for late or silent inputs (`ignore-inactive-pads`), latency, format conversion of inputs (F32
  and S16). It overrides `aggregate_one_buffer` to add each input into every output channel it contributes to, and
  `create_output_buffer` to attach the `SplitMeta`.
- The **splitter** answers the question design.md left open: an aggregator has only one source pad, so the outputs
  travel together as one wide buffer and are split into per-output pads afterwards. A `UniqueFlowCombiner` combines
  the outputs' flow returns; unlinked outputs are fine.
- The **bin** ghosts the pads: requesting `sink_N` also creates `minus_N`; releasing `sink_N` removes both. The
  sample rate is fixed at creation (`sample-rate`, default 48 kHz) rather than negotiated, because negotiating it
  race-free with pads coming and going is not possible.

**What it lacks, for us:**
- outputs are mono, and every contribution is a plain yes/no (no gain, no pan, no delay);
- the inner mix loop looks up a `Vec<bool>` per input channel per buffer and branches per sample: fine for its
  purpose, not for ours;
- adding or removing a pad changes the output buffer's channel count, so the mixer renegotiates. To make that work
  it reaches into `GstAudioAggregator`'s **private struct** (`Priv`, by layout) to work around bugs fixed in
  GStreamer 1.28.3 (MR 11477/11482/11484) and one not yet merged (MR 11486). Struct-layout hacks are fragile, and
  we must run on 1.24 (step 1 has to settle this);
- nothing after the mix (where a shared reverb would go).

**What we take:** the bin + mixer + splitter + meta structure, the pad handling and the tests' approach. Files we
copy keep their MPL-2.0 header and their authors' copyright lines, with ours added (MPL-2.0 section 3.4). We credit
the origin in the README.

**What we change (the plan below):** stereo outputs (2 channels per listener); per-path coefficients (gains, delay,
reflection taps) instead of yes/no; a short history per input (a ring buffer) so delays and reflections can read
older samples; a stage after the mix for the shared reverb (`AggregatorImpl::finish_buffer`, before the buffer goes
to the splitter); the coefficients computed from pad properties, outside the audio path.

## 2a. Also studied: gst-plugins-bad `audiomixmatrix` (what SpatialAudioServer uses today)

SpatialAudioServer's `matrix` engine: the room's voices go into one `audiointerleave` (a silence slot plus one slot
per participant, every slot requested up front so the width never changes and nothing renegotiates), then a `tee`,
then per listener `queue → audiomixmatrix` (a 2 × N gain matrix) `→ Opus encoder`. The 1.28 source is in
SpatialAudioServer's `docs/reference/gstreamer/subprojects/gst-plugins-bad/gst/audiomixmatrix/`. (The separate
`docs/reference/gst-plugins-bad` clone is the old standalone repository, archived at 1.19.2 in 2021, before
GStreamer moved to one repository; its audiomixmatrix lacks the sparse path below.)

**What it is:** a `GstBaseTransform`: one interleaved stream in, one out, `out = matrix × in` per sample frame,
coefficients −1 to 1, at most 64 channels each side. The 1.28 version picks a sparse loop (only non-zero
coefficients) or a dense one, once, when caps are set.

**What it teaches us** (its code is LGPL C: ideas only, nothing copied into this MPL crate):
- **Updates need to be safe in the element.** Its `matrix` setter frees and rebuilds the tables with no lock, so
  setting it while playing can crash; SpatialAudioServer applies every change from a buffer probe on the streaming
  thread to stay safe. Ours: property setters only write pending values; the streaming thread takes them at a
  buffer boundary (swap under a short lock, never held while mixing).
- **No hidden modes.** The sparse/dense choice is made once at caps time, and a later matrix on the other side of
  the 50 % line makes the sparse loop output silence; SpatialAudioServer works around it by using 1e-9, never 0, for
  "off". Ours: decide per buffer which inputs to skip, with no mode to get stuck in.
- **Ramps.** It jumps to new coefficients at a buffer boundary, which clicks or "zips" while someone moves or turns.
  Ours ramps (section 3).
- **Cost shape.** Every listener's matrix reads the whole room-wide buffer, silent slots included: listeners × 2 ×
  slots multiply-adds per sample, plus a tee branch, queue and buffer copy per listener. Ours handles each input
  once, writes its contribution into every listener's pair, and skips silent or inactive inputs entirely, so
  the cost follows the number of talkers (what `matrix-shared` gets by sharing encoders).
- **Fixed width works.** The interleave's fixed slots (and `Media:MaxParticipants`) show that a fixed capacity, as
  in step 1 option (b), is workable in practice.
- **Range.** Coefficients are limited to ±1 (no boost); ours allows gains above 1, as SpatialAudioServer's
  per-person volume does.

## 3. The element's interface (proposed)

```text
gst-launch-1.0 acousticmultimixer name=m room=office \
  audiotestsrc freq=440 ! m.sink_0   m.sink_0::x=0 m.sink_0::y=2 \
  audiotestsrc freq=220 ! m.sink_1   m.sink_1::x=3 m.sink_1::y=0 \
  m.mix_0 ! audioconvert ! autoaudiosink
```

- **Pads:** `sink_%u` request pads (mono, F32 or S16, at the element's `sample-rate`); each one creates a
  `mix_%u` sometimes pad (stereo F32 or S16): what participant N hears. Release `sink_N` to remove both.
- **Per-participant properties, on `sink_%u`:**
  - `x`, `y`, `z`, `yaw` (and later `pitch`): where the voice comes from and which way it faces (facing has no
    effect until the geometry work);
  - `listener-x`, `listener-y`, `listener-z`, `listener-yaw`: where this participant hears from. SpatialAudioServer
    keeps the speaking and listening poses apart (SL viewers can hear from their camera); by default they follow
    the speaking pose;
  - `gain` (0–10, applies to everyone hearing this voice), `mute`;
  - `trims`: what this participant, as a listener, has done to others' voices: a structure of gains by speaker
    pad, e.g. `trims,sink_0=0.0,sink_2=0.5` (0 = muted for them only, 1 = unchanged, up to 10), multiplied onto
    the computed level. SpatialAudioServer's per-person mute and volume map onto it;
  - listen-only and talk-only participants need nothing special: an unfed `sink_N` contributes silence (and is
    skipped), an unlinked `mix_N` is not mixed at all.
- **Element properties:** `sample-rate` (construct-only); the distance model (`distance-model` = inverse,
  exponential, linear; `reference-distance`, `rolloff`, `max-range`), matching SpatialAudioServer's `Media:Spatial`
  settings and profiles; `room` (preset name: `none`, `small-room`, `office`, `hall`, `cave`, `outdoors`, …);
  `reverb-level`; the aggregator properties `minus1mixer` already proxies (`latency`, `force-live`, …).
- Property changes only mark the coefficients as stale. They are recomputed once per output buffer, outside the
  inner loops, and the audio path ramps from the old values to the new ones across the buffer (no clicks).

## 4. Steps

Each step ends with tests passing, `gst-launch-1.0` examples that work, and the numbers written down.

### Step 0: set-up
- Install Rust through rustup (stable, clippy, rustfmt), then `cargo install cargo-c`. GStreamer 1.28.2's
  development files are already installed on the development machine.
- Crate skeleton: `Cargo.toml` (cdylib + rlib, gstreamer-rs from crates.io with the `v1_24` feature), `build.rs`,
  `src/lib.rs` registering the plugin, `cargo build` → `GST_PLUGIN_PATH=target/debug gst-inspect-1.0 acousticmultimixer`.
- Repository conventions as SpatialAudioServer: `main` + `develop`, CHANGELOG.md with an Unreleased section,
  headers on every source file (`Copyright 2026 Simon Annetts/Jak Daniels`, `SPDX-License-Identifier: MPL-2.0`),
  `cargo fmt` and `cargo clippy -D warnings` clean.
- CI (GitHub Actions): fmt, clippy, tests, on Ubuntu 24.04 (GStreamer 1.24) and 26.04 (1.28), in containers like
  SpatialAudioServer's `sas-ci` images. Windows later (step 6).

### Step 1: the template, working on 1.24 and 1.28
- Bring in `minus1mixer` (mixer, splitter, meta, bin, tests) under our name, unchanged in behaviour.
- Run its tests on 1.24 and 1.28. The private-struct hacks are the risk: `Priv`'s layout and the bugs differ by
  version.
- **Decide how pad changes are handled**, by trying:
  - (a) keeping its renegotiation, with the hacks only where needed (by run-time version check), or
  - (b) no renegotiation: a fixed output capacity (`max-participants`, default 32 → 64 output channels),
    with unused slots silent and skipped by the splitter. Simpler and version-proof, at the cost of a wider
    buffer (64 × 4 bytes × 480 frames ≈ 123 KB per 10 ms, nothing compared with the mixing).

  Expectation: (b). It also suits SpatialAudioServer, whose rooms have a fixed maximum (32) and whose
  `audiointerleave` already uses fixed slots for the same reason (section 2a).

### Step 2: stereo, distance and pan (parity with SpatialAudioServer's `matrix` engine)
- Outputs become stereo; contributions become left/right gains per (speaker, listener) path, computed from the
  pad properties with the distance model and a constant-power pan from the angle relative to the listener's yaw.
- A tight inner loop: per input buffer, per listener, two multiply-adds per sample over slices (no lookups, no
  branches per sample), so it vectorises. Gain ramps across the buffer when coefficients change.
- Tests (with `gst_check::Harness` and `appsrc`/`appsink`): a listener never hears themselves; gains and pan
  match the formula for known positions; pads added and removed while playing; silence when alone.
- Benchmark: 32 participants with 3 and with 32 talking, CPU per path, against SpatialAudioServer's `matrix`
  engine (audiointerleave + audiomixmatrix) on the same machine.
- **Here SpatialAudioServer can start using it** (issue #8 there): an `acoustic` engine, offered only when the
  element is installed, run through the existing engine test suites. Same sound as `matrix`, so any difference is
  plumbing, not acoustics.

### Step 3: direction cues (ITD, ILD, distance colour)
- **ITD:** the far ear hears the voice up to ~0.6 ms later, read from the input's history ring with a fractional
  (interpolated) delay. When the listener turns, the delay must glide, not jump (a jump clicks; a fast glide
  bends pitch slightly, which is acceptable and what real heads do).
- **ILD / head shadow:** the far ear also loses high frequencies, not just level: a one-pole low-pass on the far
  ear's path, its cut-off from the angle.
- **Distance colour:** distant voices lose high frequencies (air absorption) and gain more room sound: a
  low-pass by distance. This and the head shadow are per-path filters, so measure their cost against step 2.
- Listening tests in SpatialAudioServer rooms with the load tool's voices (`--audio voices --spatial`).

### Step 4: early reflections and room presets
- Per path, a few taps (4 to start) from the speaker's history ring: delay and gain from the room preset and the
  distance, panned progressively wider (design.md, "Reflection positioning").
- Presets as data (delays, gains, absorption, reverb settings), so new rooms need no code.
- The history ring grows to the longest tap (a cave's 430 ms ≈ 20,600 samples per input at 48 kHz: 82 KB, fine).

### Step 5: shared late reverb
- One reverb per element (a feedback delay network or a Freeverb/Dattorro-style tank: no FFT needed), fed by a
  send from every voice, added to every output.
- **To solve first: mix-minus for the reverb.** A tail made from everyone's voices contains the listener's own
  voice, which they would then hear back ~150–300 ms late through the network: an echo. Options:
  - one reverb per *talker* (few at a time, as `matrix-shared` already exploits in SpatialAudioServer); each
    listener's tail is the total minus their own talker's tail (reverbs are linear, so this is exact). Cost grows
    with talkers, not listeners;
  - accept it at a very low level (it is diffuse), or leave it out for the person speaking;
  - a reverb per listener for small rooms only.

  Measure the first option; it keeps the design's "shared" saving and stays correct.
- Denormals: flush to zero (or add a tiny offset) in decaying tails.

### Step 6: packaging and releases
- As design.md "Packaging": `.deb`/`.rpm` (`gstreamer1.0-acousticmultimixer`, Fedora `gstreamer1-acousticmultimixer`)
  for x64 and arm64, built against 1.24, installed into the system plugin directory; SpatialAudioServer's package
  *recommends* it.
- Windows: an MSVC build against GStreamer 1.28; the DLL goes into SpatialAudioServer's MSI beside its embedded
  GStreamer plugins.
- Workflows on the SpatialAudioServer pattern: versions from tags, drafts from `develop`, a release when a pull
  request into `main` is merged, the repository made public before the first release.

### Later
- Room geometry and image-source reflections (design.md "Future geometry support").
- HRTF for the nearest or loudest voices (look at gst-plugins-rs's `hrtfrender` first), occlusion, materials.
- Offering it upstream: once it is working and tested, documented, and (ideally) used beyond SpatialAudioServer,
  a merge request to gst-plugins-rs. By then `minus1mixer` may have moved on; keep our changes easy to compare with
  it.

## 5. Testing and measuring
- Unit tests for the DSP pieces (coefficients, ramps, delay interpolation) as plain Rust tests, without GStreamer.
- Element tests with `gst_check` harnesses: known inputs in, exact outputs checked (gains, delays in samples, taps).
- Benchmarks (`cargo bench` or a small example program): one element, N inputs, M talking, CPU per second of
  audio; kept in `docs/benchmarks.md` with the machine, as SpatialAudioServer does.
- End to end through SpatialAudioServer: its engine suites with the plugin installed, and the load tool for real
  rooms and listening.

Rough cost at step 4, for scale: 32 listeners × 31 paths × (1 direct + 4 taps) × 2 ears × 48,000 samples/s
≈ 480 million multiply-adds per second, under one core with SIMD. Reverb and filters come on top.

## 6. Open questions
- Mono output for listeners who want it (phone-style clients)?
- SpatialAudioServer's tethering (SL viewers' own positions used only near the simulator's) stays in the server:
  it passes the resulting positions. Its `meeting` profile is the `linear` model (Vivox's curve); check by ear that
  the element's models reproduce the server's profiles before switching rooms over.
