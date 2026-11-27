# acousticmultimixer: plan

A GStreamer element, written in Rust, that mixes N mono voices into one personal stereo mix per listener (mix-minus),
placing each voice in a shared virtual acoustic space: distance, direction (level and time differences between the
ears), early reflections and a shared late reverb, with room presets. Its first user is
[SpatialAudioServer](https://github.com/JakDaniels/SpatialAudioServer) (issue #8 there), as a new mix engine for
spatial rooms; the aim is for it to be generally useful and, in time, to be offered to gst-plugins-rs.

- The original design, with the DSP ideas and the cost analysis: [docs/design.md](docs/design.md). This plan
  holds the decisions; where they differ, this plan wins.
- Status: **step 5 done** (2026-11-19): late reverb per voice, on top of step 4's early
  reflections and room presets, step 3's head model and step 2's distance and pan; SpatialAudioServer's `acoustic` engine uses the element, and its rooms choose a preset.
  Open work is in this repository's GitHub issues (on the shared project board,
  https://github.com/users/JakDaniels/projects/1); this plan keeps the design and the decisions, and links to them:
  step 5, late reverb (#1); the silence gate (#2); geometry (#3); step 6, packaging (#4); step 7, room
  parameters (#7) and dimensions (#8).

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
| Orientation | `yaw`, `pitch`, `roll` in degrees (not yaw alone): SpatialAudioServer orients listeners by quaternion, and pitch and roll change the pan; conversion in section 3 | 2026-11-18 |
| Pad roles | one kind of participant: every `sink_N` has its `mix_N`; listen-only = `sink_N` never fed, talk-only = `mix_N` never linked (skipped) | 2026-11-18 |
| Pad changes | fixed width, option (b) of step 1: construct-only `max-participants` (default 32, up to 256), pads numbered below it, the mix always that wide, so pads come and go without renegotiating; no access to AudioAggregator's private struct | 2026-11-18 |

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

## 3. The element's interface

```text
gst-launch-1.0 acousticmultimixer name=m sink_1::y=4 sink_2::x=8 \
  audiotestsrc freq=440 ! m.sink_0 \
  audiotestsrc wave=ticks ! m.sink_1 \
  audiotestsrc freq=220 ! m.sink_2 \
  m.mix_0 ! audioconvert ! autoaudiosink
```

- **Pads:** `sink_%u` request pads (mono, F32 or S16, at the element's `sample-rate`), numbered below
  `max-participants`; each one creates a `mix_%u` sometimes pad (stereo F32 or S16): what participant N hears.
  Release `sink_N` to remove both. With more than one output linked, a `queue` after each. Pad properties can be
  set in gst-launch inside the element's description, as with `compositor` (`sink_1::y=4`): the element exposes
  its sink pads as its children (GstChildProxy).
- **Per-participant properties, on `sink_%u`:**
  - `x`, `y`, `z`, `yaw`, `pitch`, `roll`: where the voice is and which way it faces (SL/OpenSim axes: x east,
    y north, z up; degrees; all 0 faces +x; yaw turns left, pitch looks up, roll lowers the right ear). Setting any
    of them sets `positioned`; a participant without a position is heard centred at full volume and hears everyone
    that way (as SpatialAudioServer does for participants without a pose). Facing has no effect on the voice yet;
  - `listener-x` … `listener-roll`: where this participant hears from (SL viewers can hear from their camera);
    setting any sets `listener-separate`, otherwise they hear from the speaking pose;
  - from a quaternion (x, y, z, w), as SpatialAudioServer holds them: yaw = atan2(2(wz + xy), 1 − 2(y² + z²)),
    pitch = −asin(2(wy − zx)), roll = atan2(2(wx + yz), 1 − 2(x² + y²)) (tested against its rotation code);
  - `gain` (0–10, for everyone hearing this voice), `mute`;
  - `trims`: what this participant, as a listener, has done to others' voices: a structure of gains by speaker
    pad, e.g. `trims,sink_0=0.0,sink_2=0.5` (0 = muted for them only, 1 = unchanged, up to 10), multiplied onto
    the computed level. SpatialAudioServer's per-person mute and volume map onto it. A participant leaving is
    removed from everyone's trims, so a newcomer given the same pad number starts clean;
  - listen-only and talk-only participants need nothing special: an unfed `sink_N` contributes nothing, an
    unlinked `mix_N` is not mixed.
- **Element properties:** `sample-rate` and `max-participants` (construct-only); the distance model
  (`distance-model` = `inverse`, `exponential` (default), `linear`; `reference-distance` 2, `rolloff` 1,
  `max-range` 60: SpatialAudioServer's `outdoor` profile; its `meeting` profile is linear, 10, 2, 60);
  `direction-model` (`head`, the default: the far ear later, quieter and duller; `pan`: a constant-power stereo
  pan), `air-absorption` (dB per metre at 8 kHz, default 0.1; `pan` with 0 is SpatialAudioServer's `matrix`
  exactly), `rear-shadow` (`head` only: the low-pass cut-off for a voice straight behind, default 4000 Hz) and
  `room` (early reflections and reverb: `outdoor`, the default, none; `tiny-room`, `small-room`, `big-room`, `hall`,
  `long-hall`, `cave`, `cathedral`); the aggregator properties
  `minus1mixer` already proxies (`latency`, `force-live`, …); `reverb-level` (#1) and `taper`.
- Property changes only mark the scene as changed. The gains are recomputed at most once per output buffer, and a
  changed path ramps linearly from the old value to the new one across that buffer (10 ms by default). A new
  participant, or an output just linked, starts at its values: there is nothing to click from.
- Setting a pose takes several property sets (x, y, z, …), and the mixer may take the scene between two of them;
  the result is one buffer ramping towards an in-between pose, inaudible at the rates poses change. If it ever
  matters, a `pose` property taking all six at once.

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

**Done (2026-11-18).** Taken from gst-plugins-rs commit 916404886 (2026-09-11). Renamed throughout: element
`acousticmultimixer`, output pads `mix_%u`, and the GTypes, meta and debug categories (`acousticmultimixer`,
`acousticmultimixer-mixer`, `acousticmultimixer-splitter`), so the two plugins can be loaded side by side.

What we found:
- **The workarounds are needed under (a).** The private-struct prefix they read (`GstAudioAggregatorPrivate` up to
  `current_buffer`) is identical in 1.24.0 and 1.28.2, so (a) does work on both. But in a live test with inputs
  joining and leaving every few milliseconds, the "output buffer pending" workaround fired about 200 times in
  2 seconds: without it, every join or leave risks a renegotiation in the middle of an output buffer.
- **(b) needs none of them.** With the width fixed, `negotiate` still runs whenever an output is linked (the
  RECONFIGURE travels upstream), but with unchanged caps, and `negotiated_src_caps` does nothing when the caps are
  equal (same code in 1.24.0 and 1.28.2). So the mixer uses only public API (`parent_negotiated_src_caps`), and a
  configuration of the same width is taken from the next output buffer, without renegotiating.
- **Cost of the width** (10 minutes of audio, 3 participants, release build, user CPU, sources included): (a) 7.6 s;
  (b) at width 3: 7.7 s, 32: 9.6 s, 256: 25.4 s. The extra is the template's inner loop visiting every output
  channel for every sample; step 2's loop skips unused outputs, so this mostly goes away.
- All outputs are pushed from one thread: with more than one linked, each needs a `queue` (as after a `tee`), or
  the first sink to preroll blocks the rest. Documented on the element.

Decision: **(b)**. Tests: the template's three (spectrum and exact sums), plus pads added and removed while
running with exact sums after each change, a live churn test (150 joins and leaves while audio flows, no errors,
everyone still receiving), and capacity (numbers reused, refusal when full or out of range); 40 repeat runs clean.

### Step 2: stereo, distance and pan (parity with SpatialAudioServer's `matrix` engine)
- Outputs become stereo; contributions become left/right gains per (speaker, listener) path, computed from the
  pad properties with the distance model and a constant-power pan from the angle relative to the listener's yaw.
- A tight inner loop: per input buffer, per listener, two multiply-adds per sample over slices (no lookups, no
  branches per sample), so it vectorises. Gain ramps across the buffer when coefficients change.
- Tests (with `gst_check::Harness` and `appsrc`/`appsink`): a listener never hears themselves; gains and pan
  match the formula for known positions; pads added and removed while playing; silence when alone.
- The splitter's GAP events for outputs with nothing assume mono (a FIXME in the template): fix with stereo.
- Benchmark: 32 participants with 3 and with 32 talking, CPU per path, against SpatialAudioServer's `matrix`
  engine (audiointerleave + audiomixmatrix) on the same machine.
- **Here SpatialAudioServer can start using it** (issue #8 there): an `acoustic` engine, offered only when the
  element is installed, run through the existing engine test suites. Same sound as `matrix`, so any difference is
  plumbing, not acoustics.

**Done (2026-11-18).**
- `spatial.rs` holds SpatialAudioServer's formulas (`SpatialPolicy.cs`: the three distance models, the pan from
  the angle, constant-power stereo with the centre at unity on both sides), with plain Rust tests including its
  quaternion rotation against our yaw/pitch/roll over 1000 random orientations; `scene.rs` the participants and
  the gain matrix; `pad.rs` the sink pad type and its properties.
- The mixer's output is still one wide buffer, now two channels per participant, and its inner loop is per input
  sample one multiply-add over a contiguous run of the output frame (`out[lo..hi] += x × gains`), which
  vectorises; the run is narrowed per input to the listeners who hear it, and an input heard by nobody is skipped.
- The SplitMeta is gone: with a fixed layout, output N is always channels 2N and 2N + 1. All outputs get every
  buffer, so the splitter no longer sends GAP events (the mono FIXME went with them).
- Element tests with exact values from the formulas: level and pan from positions, a move ramping over one buffer
  (checked frame by frame), gain, mute and trims, a separate listener pose, the distance model and range, alone =
  silence, talk-only, and the gst-launch pad property syntax. 30 repeat runs clean.
- **Benchmark** (`scripts/bench.py`, [docs/benchmarks.md](docs/benchmarks.md)): 32 participants, live, on a
  4-core Pentium Silver N6005: about 21 % of a core against 26–31 % for SpatialAudioServer's `matrix` mixing
  (audiointerleave + tee + 32 × queue + audiomixmatrix), without the encoders either way. Our cost hardly changes
  with the number talking: the mixing arithmetic is now small, and what remains is GStreamer's per-buffer work
  (the aggregator's input handling: a plain `audiomixer` with the same 32 inputs costs 7 %; 32 output pushes and
  their queues, which every per-listener design has). Longer output buffers (20 ms) would halve that, at the cost
  of latency; worth trying alongside SpatialAudioServer's 40 ms Opus frames.
- Not yet: recomputing only the paths that changed (the whole matrix is recomputed when anything changes:
  1,024 paths at 32 participants, about 50 µs; at 256 participants it would matter).

### Step 3: direction cues (ITD, ILD, distance colour)
- **ITD:** the far ear hears the voice up to ~0.6 ms later, read from the input's history ring with a fractional
  (interpolated) delay. When the listener turns, the delay must glide, not jump (a jump clicks; a fast glide
  bends pitch slightly, which is acceptable and what real heads do).
- **ILD / head shadow:** the far ear also loses high frequencies, not just level: a one-pole low-pass on the far
  ear's path, its cut-off from the angle.
- **Distance colour:** distant voices lose high frequencies (air absorption) and gain more room sound: a
  low-pass by distance. This and the head shadow are per-path filters, so measure their cost against step 2.
- Listening tests in SpatialAudioServer rooms with the load tool's voices (`--audio voices --spatial`).

**Done (2026-11-18), listening tests to come.**
- `direction-model=head` (the default): the near ear as before (gain from distance); the far ear later by
  Woodworth's ITD for a spherical head, (r / c)(θ + sin θ), up to 0.66 ms at the side; quieter by up to 6 dB
  (ILD; design.md suggested 8, less here because the shadow adds to it); and low-passed (head shadow), the cut-off
  falling from none ahead to 1.8 kHz at the side. θ is the lateral angle, from the same pan as before (sin θ =
  |pan|), so front and back are still alike. `direction-model=pan` keeps step 2's stereo pan. The constants
  (`spatial.rs`) are first guesses, to tune by ear.
- `air-absorption` (default 0.1 dB per metre at 8 kHz, roughly real air): a one-pole low-pass on both ears whose
  loss at 8 kHz is that times the distance (30 m: cut-off 8 kHz; 60 m: 4.6 kHz). The far ear takes the lower of
  its two cut-offs.
- `rear-shadow` (added after the first listening test, where front and back sounded the same): with `head`, a voice
  behind is low-passed in both ears, as the outer ears shade sounds from behind; the cut-off falls from none at the
  side to 4 kHz straight behind (about 3 dB down at 4 kHz, 7 dB at 8 kHz), following the cosine of the angle from
  straight back. Listening with pink noise (the load tool's `--audio pink-noise`): front and back now differ, and
  it sounds more realistic.
- `dsp.rs`, free of GStreamer: per path and ear a gain, a delay and a one-pole low-pass; gains and delays ramp
  across the buffer when they change (so a turning listener's delays glide); each speaker keeps a short history
  (the longest ITD plus two samples), cleared, with the filters, after a gap in its input. A speaker whose paths
  are all plain gains (no delay, no filter: unpositioned, or `pan` without air) keeps step 2's vectorised loop;
  otherwise the loop runs across the ears of each output frame (the filters feed back in time, but the ears are
  independent), with each ear's whole-sample offset and fraction worked out once per buffer when the delays are
  not changing.
- Cost (as fast as possible, 32 participants, all producing audio): `pan` 24 % of a core, `head` first 101 %, then
  92 % (loop across ears), 57 % (offsets once per buffer), 44–47 % (no bounds checks in the inner loop). Live, 32
  participants: 47–51 %, against 28–31 % for SpatialAudioServer's `matrix` mixing; with the 29 non-talkers
  silent (GAP) 22 % against 27 %. What remains is reading each ear's delayed sample (a gather: no SIMD on this
  CPU) and the filter.
- Tests: the ear formulas; the DSP (fast loop, a delayed ear reading across buffers, a gap clearing the history,
  the one-pole's step response, gain and delay ramps); through the element, the head model's far ear silent for
  the ITD, then rising through the shadow and settling 6 dB down. Step 2's exact-value tests run with `pan` and no
  air.
- SpatialAudioServer: its spatial suite's "on the left" check required the far ear below −14 dB (a stereo pan's
  silence); now −3 dB, which both models meet.
- **To decide ([#2](https://github.com/JakDaniels/acousticmultimixer/issues/2)):** a silence gate. Participants who are not talking still produce audio (decoded comfort noise), and
  with `head` every one of them costs as much as a talker: 47 % against 22 % when they are silent (GAP). Skipping
  inputs whose buffer peaks below a threshold would make the cost follow the talkers, but changes what is heard
  (the noise floor goes) and needs a threshold that never cuts quiet speech.

### Step 4: early reflections and room presets
- Per path, a few taps (4 to start) from the speaker's history ring: delay and gain from the room preset and the
  distance, panned progressively wider (design.md, "Reflection positioning").
- Presets as data (delays, gains, absorption, reverb settings), so new rooms need no code.
- The history ring grows to the longest tap (step 4's cave, 430 ms ≈ 20,600 samples per input at 48 kHz: 82 KB, fine; its taps now end at 110 ms).

**Done (2026-11-18), listening-tested from Firestorm** (each preset sounds different; changing it while people talk
is seamless).
- `room` property; presets in `rooms.rs` as data: four taps each (delay after the direct sound, level at the
  reference distance) and a one-pole low-pass for the walls. small-room 3/7/11/17 ms, −9…−19 dB, 6 kHz; hall
  15/25/40/70 ms, −10…−19 dB, 5 kHz; cave 80/140/220/430 ms, −8…−20 dB, 3 kHz (25/45/70/110 ms since step 5, which
  also added four presets: the current values are in step 5); outdoor none.
- The taps' delays are the same for every listener, so not per path as first planned: each speaker's
  reflections are made once, from a ring of its input (a power of two above the longest tap, allocated when the
  room changes), as three signals: a directional part (tap k contributes 1 − (k + 1)/n of its level) and left and
  right diffuse parts ((k + 1)/n; their delays 8 % apart, alternately earlier on each side, for width), each
  low-passed for the walls. A path adds them with two levels per ear: the directional part times the ear's share
  of the direct sound, the diffuse part the same in both ears.
- Reflections fall off with distance at half the direct sound's rate in dB (level √(distance gain)), so a far
  voice is relatively roomier, a distance cue. Close up they add to the direct sound (small room at 2 m: about
  5 dB louder than outdoors).
- Cost: +10 % of a core at 32 participants (48 → 58 %, as fast as possible), the same for every preset; mixing
  the reflections per listener pair (left and right columns together) keeps that loop simple enough to vectorise
  (it was +25 % per column).
- Known limit: a speaker's input that stops (GAP buffers) clears its ring, cutting its echo tail. SpatialAudioServer's
  decoders keep producing audio between words, so it does not arise there.
- Next, agreed order: step 5 (a feedback delay network per voice; each listener hears the room's total minus their
  own, exact since reverb is linear; the late field is the same everywhere in a room, so sharing it is right for
  bounded interiors, and outdoor has none), then the silence gate, then geometry (first-order image sources for a
  box room from its dimensions: per-path delays, affordable once only talkers are mixed).

### Step 5: shared late reverb (#1)
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

**Done (2026-11-19), listening-tested from Firestorm** (after the tuning below; no stutter with six talking
bots).
- Changed from the above (agreed 2026-11-19): not a room total minus each listener's own tail, but each voice's
  tail added to its two diffuse reflection signals. It then reaches each listener at that path's diffuse level:
  it falls with distance like the reflections (in a region-wide voice room a constant-level tail would
  bring in everyone's voice from 100 m away), per-person trims and mute apply, the speaker never hears their own,
  and it costs nothing per listener, only the network per voice.
- `reverb.rs`: a feedback delay network of eight lines (lengths the room's size times spread ratios, rounded to
  distinct primes), mixed by an 8-point Hadamard matrix; each line's feedback is a one-pole low-pass whose gains at
  DC and at the top of the band give the room's bass and treble decay times (Jot's approach, simplified). Input
  fed with mixed signs; the left ear hears the even lines, the right the odd ones, so the tail is wide. The input
  gain is worked out from the level, decay time and mean line length (tested within 1.5 dB). Values below 1e-15
  in the filters are flushed to zero, so fading tails never reach denormals and end in exact silence.
- Presets (bass decay / treble decay / level against the direct sound at the reference distance / size):
  small-room 0.4 s / 0.15 s / −15 dB / 12 ms; hall 1.1 s / 0.35 s / −15 dB / 45 ms; cave 1.8 s / 0.5 s / −13 dB /
  70 ms. The first values (hall 1.6 s at −9 dB, cave 3 s at −6 dB) were too strong and long by ear.
- The cave's taps were step 4's 80/140/220/430 ms: walls 14–74 m away, gaps of 60–210 ms heard as separate
  echoes. Now 25/45/70/110 ms (walls about 4–19 m away, a chamber 25–35 m across; gaps under 50 ms blend), with a
  70 ms network size to match; the long dark tail is what sets it apart from the hall.
- Three more presets (2026-11-19), to be tuned by ear: tiny-room (a closet or bathroom: taps 1.5/3.5/5.5/8 ms,
  7 kHz; tail 0.3 s / 0.12 s / −16 dB / 6 ms), big-room (a classroom or large office: 8/15/24/36 ms, 5.5 kHz;
  0.7 s / 0.25 s / −15 dB / 25 ms) and long-hall (a gallery or corridor: near side walls at 5/12 ms, far ends at
  45/85 ms, 5 kHz; 1.4 s / 0.4 s / −15 dB / 40 ms). Then cathedral (vast stone and glass: 20/38/60/90 ms, 4 kHz;
  3 s / 1.2 s / −12 dB / 90 ms: the longest and brightest tail, shortened from a real one's 5–8 s for speech).
- Close up (2026-11-19): inside the reference distance the direct sound stays at full volume, so the voice-to-room
  balance did not change there at all. Now the room sound (reflections and reverb) is turned down instead: 6 dB
  per halving of the distance, to −12 dB at 0.5 m (`NEAR_FLOOR`), so someone right next to you stands out from
  the room. From `near-distance` (2 m), its own setting: the reference distance also sets the direct sound's
  curve, and the `meeting` profile's 10 m would turn the room far down. Overall loudness is still capped. Not yet tried, if moving away should shift the balance more: a
  `ROOM_FALLOFF` of 0.6 (the room +7 dB against the voice at 16 m instead of +4.5 dB; far voices a little
  louder).
- Reflections and reverb now fall at three quarters of the direct sound's rate in dB (`ROOM_FALLOFF`), not half:
  at half, a voice 58 m away was still at −15 dB of room sound (direct −29 dB) until the 60 m cut-off.
- Denormals: the one-pole filters (per-ear, from step 3, and the walls', from step 4) fed exact silence after
  sound decayed into denormals and could stay on one for good; with six voices that cost 40 times as much (50 %
  of a core live, stuttering, worst while walking, as ramps take the slower loop). The benchmark missed it: its
  quiet sources are never exactly zero. Fixed by flushing denormals to zero while mixing (`FlushDenormals`:
  MXCSR FTZ+DAZ on x86-64, FPCR FZ on ARM64, restored afterwards); the network also flushes its own filters.
- `taper` (0.75): every distance curve fades out (raised cosine) from that fraction of `max-range` to silence at
  it; a voice was still clearly heard just inside the range, then cut off. SpatialAudioServer's distance policy
  does the same (`Media:Spatial:Taper`).
- `reverb-level` (dB against the preset, −60…+12; −60 turns it off and frees its state). Memory: a cave's lines
  are about 42,000 samples per voice (170 KB; 5.4 MB at 32).
- Cost: +9 % of a core at 32 participants (54 → 64 %, as fast as possible), the same for every preset; within
  the 10–15 % estimated.
- Same known limit as the reflections: GAP input clears a voice's network, cutting its tail.

### Step 6: packaging and releases (#4)
**Under way (2026-11-19).** The repository is public (2026-11-19), set up as SpatialAudioServer's: default branch
`main`; rulesets on `main` (pull request, merge commits only, checks `test-1.28` and `test-1.24`) and `develop` (no
deletion or force-push); the release labels `minor`, `major`, `publish`.
- `scripts/version.sh`: the version from the tags, as MinVer gives SpatialAudioServer (0.1.1-alpha.0.N after
  v0.1.0); `scripts/with-version.sh` writes it into Cargo.toml for a build only (gst-inspect-1.0 shows it).
- `scripts/build-packages.sh`: `.deb` and `.rpm` with nfpm (`packaging/linux/`), for the machine's architecture,
  stripped; `/usr/lib/<triplet>/gstreamer-1.0/` and `/usr/lib64/gstreamer-1.0/`; built on Ubuntu 24.04 (GStreamer
  1.24, glibc 2.39). `scripts/test-packages.sh` installs one and runs a short mix.
- `packages.yml` (called by `drafts.yml` and `release.yml`, both adapted from SpatialAudioServer's): Linux on x64
  and arm64 runners (free now the repository is public), tests on Ubuntu 24.04 and 26.04, Debian 13 and Fedora on
  both architectures, and the Windows DLL against GStreamer 1.28.8 (the installer's `devel` type), loaded and run;
  the release (zip for Windows, SHA256SUMS) only once everything passed. Workflow artefacts between jobs.
- Proposed (2026-11-19): no token to start with. Publishing a release would not update SpatialAudioServer's apt/dnf
  repositories by itself: its `pages` workflow (once it takes the plugin's latest release) is run by hand, or with
  its next release. A token and a `repository_dispatch` can be added later.
- As design.md "Packaging": `.deb`/`.rpm` (`gstreamer1.0-acousticmultimixer`, Fedora `gstreamer1-acousticmultimixer`)
  for x64 and arm64, built against 1.24, installed into the system plugin directory; SpatialAudioServer's package
  *recommends* it.
- Windows: an MSVC build against GStreamer 1.28; the DLL goes into SpatialAudioServer's MSI beside its embedded
  GStreamer plugins.
- Workflows on the SpatialAudioServer pattern: versions from tags, drafts from `develop`, a release when a pull
  request into `main` is merged, the repository made public before the first release.
- Agreed (2026-11-18): CI (`ci.yml`) also runs on pull requests, and a ruleset on `main` requires it, as in
  SpatialAudioServer. The packages are built here and attached to this repository's release (its own versions;
  usable without SpatialAudioServer). On a release, a `repository_dispatch` to SpatialAudioServer has its
  `pages.yml` add them to the signed apt/dnf repositories, so the server package's *recommends* resolves there (needs
  a token secret here with access to that repository: a fine-grained token or a GitHub App). SpatialAudioServer's
  MSI build downloads the DLL from a pinned release of this repository, so each installer ships a known version.

### Step 7: room parameters (#7) and dimensions (#8)
**Decided (2026-11-27).** Rooms are tuned live from SpatialAudioServer's admin UI and in-world scripts
(SpatialAudioServer#31, os-webrtc-sas#13), not only by choosing a preset.
- **Overrides on presets**, not fully custom rooms: `room` still chooses the preset, and each of its values can be
  overridden by a property of its own (unset = the preset's). **All of them are exposed:** the taps' delays and
  levels (`reflection-delays`, `reflection-levels`: lists, so the number of taps can change), `wall-cutoff`,
  `reverb-decay`, `reverb-treble-decay`, `reverb-size`, and the constants every preset shares today:
  `room-falloff` (`ROOM_FALLOFF`), `reflection-spread` (`SPREAD`), `reflection-diffusion` (`diffuse_share`) and
  `near-floor` (`NEAR_FLOOR`). The preset's reverb level stays covered by `reverb-level`. Names and the list
  format to settle in #7; these property names are used everywhere (SpatialAudioServer's API in camelCase,
  notecards as they are).
- A read-only `room-parameters` structure gives the values in effect and where each came from (preset,
  dimensions, override), so a UI can show a preset's values before anything is changed.
- **Derived from dimensions** (#8): width, depth, height and the surfaces' absorption (bass and treble) give the
  decay times (Sabine or Eyring), the size (mean free path 4V/S), first-order tap delays and levels for a speaker
  and listener near the middle, and the walls' cutoff. Computed when the room changes, so no cost per sample.
  Precedence per value: override, then dimensions, then preset. The same dimensions later feed the per-path image
  sources (#3).
- Changes stay seamless while people talk, including those that resize memory (a longer last tap grows the
  history ring; a new `reverb-size` changes the delay lines).
- **Order:** #7, #8; then the silence gate (#2) as its own piece, so tuning rooms by ear is not mixed up with the
  gate's change to what is heard (and the gate must let a skipped voice's reflections and reverb die away, not
  cut them); then geometry (#3).

### Later
- Room geometry and image-source reflections (design.md "Future geometry support"; first-order, for a box room: #3).
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
