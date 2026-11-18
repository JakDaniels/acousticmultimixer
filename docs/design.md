> **Copied** on 2026-11-18 from SpatialAudioServer's `docs/gstreamer_acousticspace_design.md` (that copy stays as
> the original idea). The element is now called `acousticmultimixer`, not `acousticspace`; where this document and
> [PLAN.md](../PLAN.md) differ, PLAN.md holds the decision.

# Custom GStreamer Acoustic Space DSP Plugin

## Overview

The goal is to build a custom GStreamer DSP element that goes beyond traditional conferencing and creates the illusion of participants occupying a shared virtual acoustic space.

Target environments include:

- Small reflective rooms
- Offices
- Hallways
- Warehouses
- Churches
- Caves
- Outdoor spaces
- Forests

The design prioritises immersion, positional awareness, and scalability while keeping CPU requirements practical for an MCU architecture.

---

# Why a Custom Plugin?

A conventional WebRTC SFU forwards streams and lets clients perform spatialisation.

This project requires an MCU architecture where:

- Audio is mixed centrally.
- Each listener receives a personalised mix.
- Participants can occupy positions in a simulated environment.
- Acoustic effects are generated server-side.

A native GStreamer plugin is a better fit than LV2/LADSPA because the problem is fundamentally a many-input / many-output routing and acoustic simulation engine rather than a simple audio effect.

---

# Proposed GStreamer Element

## Element Name

```text
acousticspace
```

## Responsibilities

- Multiple audio inputs
- Multiple audio outputs
- Participant position tracking
- Listener orientation tracking
- Environmental modelling
- N-minus-1 mixing
- Spatial positioning
- Distance modelling
- Early reflections
- Shared reverb bus

---

# High-Level Architecture

```text
 Alice ----\
 Bob -------\
 Charlie ----> acousticspace ---> Mix for Alice
 Dave ------/                  ---> Mix for Bob
 Eve -------/                  ---> Mix for Charlie
                                ---> Mix for Dave
                                ---> Mix for Eve
```

Internally the element generates unique mixes for each participant.

---

# GStreamer Structure

The element would likely derive directly from:

```c
GstElement
```

rather than:

```c
GstAudioFilter
```

because it requires multiple sink and source pads.

## Request Pads

```text
sink_0
sink_1
sink_2
...
sink_N

src_0
src_1
src_2
...
src_N
```

Each sink pad represents one participant's microphone stream.

Each source pad delivers that participant's personalised output mix.

---

# Participant Model

Each participant maintains spatial state.

```c
struct Participant
{
    float x;
    float y;
    float z;

    float yaw;

    float gain;

    bool active;
};
```

Properties:

- Position in 3D space
- Orientation
- Gain adjustment
- Activity state

---

# Control Interface

Rather than exposing hundreds of GObject properties, participant movement should be delivered via custom events.

Example:

```text
participant-update
{
    id = 5
    x = 12.3
    y = 4.5
    z = 0.0
}
```

Typical updates:

- Position changes
- Orientation changes
- Environment changes
- Gain changes

---

# Environment Model

## Presets

Examples:

```text
small-room
office
hallway
warehouse
church
cave
outdoors
forest
```

A preset contains:

```c
struct RoomPreset
{
    float rt60;
    float hf_absorption;
    float reflection_density;
};
```

---

# DSP Pipeline

For every speaker/listener pair:

```text
Distance
   |
ILD / ITD
   |
Early Reflections
   |
Contribution Buffer
```

After all contributions:

```text
Mix
 |
Shared Late Reverb
 |
Output
```

---

# Stage 1 - Distance Modelling

Distance determines:

- Gain reduction
- High frequency attenuation
- Direct-to-reflection ratio

Possible attenuation:

```c
gain = 1 / (1 + k * distance);
```

---

# Stage 2 - Localisation

Use inexpensive binaural cues.

## ITD

Interaural Time Difference.

Typical range:

```text
0 to 0.6 ms
```

## ILD

Interaural Level Difference.

Example:

```text
Speaker on left

Left  = 0 dB
Right = -8 dB
```

This provides convincing localisation with very little CPU cost.

---

# Stage 3 - Early Reflections

The key design conclusion was that early reflections contribute more realism than expensive HRTF processing.

A low-CPU approach uses several delayed taps.

Example:

```text
Tap 1 = 4 ms   -10 dB
Tap 2 = 9 ms   -14 dB
Tap 3 = 16 ms  -18 dB
Tap 4 = 24 ms  -22 dB
```

Implementation:

```c
out = direct
    + tap4ms  * 0.32
    + tap9ms  * 0.20
    + tap16ms * 0.13
    + tap24ms * 0.08;
```

Advantages:

- Very low CPU usage
- No convolution
- No feedback loops
- Strong perception of room size

---

# Reflection Positioning

Reflections should become progressively less directional.

Example:

Direct:

```text
L = 0.8
R = 0.4
```

Reflections:

```text
L = 0.6 R = 0.4
L = 0.55 R = 0.45
L = 0.52 R = 0.48
L = 0.50 R = 0.50
```

This increases realism and room perception.

---

# Shared Late Reverb

Late reverb is largely diffuse and difficult for listeners to localise.

Therefore one shared reverb engine should be used.

```text
Participant Contributions
           |
        Mix Bus
           |
      Late Reverb
           |
        Outputs
```

Benefits:

- Very large CPU saving
- Consistent acoustic space
- Scales well with participant count

---

# Room Presets

## Office

```text
4 ms  -8 dB
7 ms  -12 dB
11 ms -15 dB
```

## Cave

```text
80 ms  -8 dB
140 ms -12 dB
220 ms -15 dB
430 ms -20 dB
```

## Outdoors

```text
Very weak reflections
Minimal reverb
Distance attenuation only
```

The timing and density of reflections create most of the perceptual difference between spaces.

---

# Future Geometry Support

Future versions could support explicit room geometry.

Example:

```json
{
  "walls": [
    {"x1":0,"y1":0,"x2":10,"y2":0},
    {"x1":10,"y1":0,"x2":10,"y2":8},
    {"x1":10,"y1":8,"x2":0,"y2":8},
    {"x1":0,"y1":8,"x2":0,"y2":0}
  ]
}
```

Using the image-source method, first-order reflections could be generated dynamically.

---

# HRTF Strategy

The conclusion was not to start with HRTF.

Version 1 should focus on:

- Positioning
- Distance
- ITD
- ILD
- Early reflections
- Room presets
- Shared reverb

Later enhancements may use:

- libmysofa
- OpenAL Soft techniques
- SOFA datasets

for premium spatial rendering.

---

# Recommended MVP

## Features

- Multiple participants
- X/Y/Z positioning
- Listener orientation
- Distance attenuation
- ITD localisation
- ILD localisation
- Four-tap early reflection engine
- Shared late reverb
- Environment presets
- N-minus-1 mixing

## Explicitly Deferred

- Full HRTF convolution
- Occlusion modelling
- Wall diffraction
- Ray tracing
- Per-participant reverb instances

This MVP should already provide a convincing illusion of participants occupying a common acoustic environment while remaining computationally practical for a server-side MCU.

---

# DSP Core Architecture and Computational Complexity

## Design Goals

The DSP engine should:

- Scale to tens or hundreds of participants.
- Support personalised listener mixes.
- Support virtual spaces and environmental acoustics.
- Minimise duplicated calculations.
- Remain CPU-efficient enough for server-side MCU deployment.

The primary challenge is that every listener hears a different version of every speaker.

---

# Naïve Approach

A naïve implementation performs complete processing for every speaker/listener pair:

```text
Speaker A -> Listener A
Speaker A -> Listener B
Speaker A -> Listener C

Speaker B -> Listener A
Speaker B -> Listener B
Speaker B -> Listener C

...
```

Complexity:

```text
O(S × L)
```

where:

- S = speakers
- L = listeners

For 50 participants:

```text
50 × 49

= 2450 signal paths
```

This quickly becomes expensive if each path includes:

- Position calculations
- Reflection generation
- HRTF
- EQ
- Reverb

---

# Recommended Processing Pipeline

The recommended architecture separates calculations into:

## Stage 1: Speaker Processing

Performed once per speaker.

```text
Input Audio
    |
Distance EQ
    |
Voice Processing
    |
Delay Buffer
```

Output:

```text
speaker_buffer[n]
```

stored for later reuse.

Complexity:

```text
O(S)
```

---

## Stage 2: Acoustic Path Generation

For each speaker/listener pair:

```text
Geometry
    |
Gain
ILD
ITD
Reflection Taps
```

This stage generates coefficients rather than processing audio repeatedly.

Example:

```c
struct AcousticPath
{
    float gain;

    float left_gain;
    float right_gain;

    int itd_samples;

    Reflection taps[4];
};
```

These coefficients change only when:

- Listener moves
- Speaker moves
- Room changes

NOT every audio block.

---

# Coefficient Caching

Most participant positions change much more slowly than the audio stream.

Audio:

```text
48000 samples/sec
```

Position updates:

```text
10-50 updates/sec
```

Therefore:

```text
Calculate acoustics
Store coefficients
Reuse during mixing
```

This dramatically reduces CPU usage.

---

# Audio Mixing Pass

Processing block:

```c
for each listener
{
    clear_output();

    for each speaker
    {
        if (speaker == listener)
            continue;

        apply_cached_path();
        accumulate();
    }
}
```

No geometry calculations occur during the audio pass.

Only:

```text
gain
delay taps
mixing
```

---

# Delay Buffer Design

Each speaker owns a circular buffer:

```c
float delay_buffer[MAX_DELAY];
```

Typical size:

```text
100 ms
```

at 48 kHz:

```text
4800 samples
```

Reflections simply read older samples:

```c
direct = buffer[now];

r1 = buffer[now - 192];
r2 = buffer[now - 432];
r3 = buffer[now - 768];
r4 = buffer[now - 1152];
```

No FFTs.

No convolution.

Just memory reads and multiply-adds.

---

# Reflection Engine

Each environment supplies a reflection preset.

Example:

```c
struct Reflection
{
    int delay_samples;
    float gain;

    float left_pan;
    float right_pan;
};
```

Office:

```text
4ms
7ms
11ms
```

Hall:

```text
15ms
25ms
40ms
70ms
```

Cave:

```text
80ms
140ms
220ms
430ms
```

These are loaded once and reused.

---

# Shared Reverb Bus

The most expensive component should be shared.

```text
Direct Paths
      |
      +----> Listener Mix
      |
      +----> Reverb Send
```

One reverb instance:

```text
all_speakers_mix
       |
   reverb
       |
 diffuse_tail
```

This avoids:

```text
participants × reverb_instances
```

which would be prohibitively expensive.

---

# Listener Orientation

Each listener stores:

```c
position
yaw
pitch
roll
```

Incoming speaker locations are transformed into listener-relative coordinates.

Example:

```text
World:
Alice  (10,0)
Bob    (20,0)

Alice turns 180°
```

No audio processing changes.

Only:

```text
pan coefficients
ITD coefficients
```

are recalculated.

---

# Optional HRTF Layer

Future versions may support:

```text
libmysofa
SOFA datasets
```

Architecture:

```text
Direct Path
     |
Simple ILD/ITD
     |
Optional HRTF
     |
Mix
```

Possible optimisation:

Apply HRTF only to:

```text
nearest speakers
or
loudest active speakers
```

while distant participants use simple stereo positioning.

This keeps CPU cost bounded.

---

# Approximate Scaling

For N participants:

Geometry updates:

```text
O(N²)
```

but only on movement events.

Audio mixing:

```text
O(N²)
```

which is unavoidable for personalised mixes.

Reflection generation:

```text
O(1)
```

per path using fixed tap counts.

Reverb:

```text
O(1)
```

using a single shared engine.

This provides practical scalability while preserving convincing spatial acoustics.

---

# Version 1 DSP Roadmap

Phase 1:

- N-minus-1 mixing
- Distance attenuation
- Stereo panning
- ILD
- ITD

Phase 2:

- Early reflection engine
- Environment presets
- Shared reverb

Phase 3:

- Room geometry
- Image-source reflections

Phase 4:

- HRTF
- Occlusion
- Material modelling

The key design principle is:

> Perform expensive acoustic calculations only when geometry changes, and keep the real-time audio path limited to gain, delay, interpolation and accumulation.

---

# Development and Packaging

*Added 2026-11-16. Nothing is built yet.*

GStreamer is designed for plugins built outside its own source tree: a plugin is a shared library (`.so` / `.dll`)
found in GStreamer's plugin directories or on `GST_PLUGIN_PATH`. So `acousticspace` can be developed and shipped on
its own, and offered to GStreamer once it has proved itself.

## Its own project

- A separate repository (e.g. `gst-acousticspace`), not part of SpatialAudioServer. That keeps it usable by anyone,
  and it is what GStreamer would expect to see when it is proposed.
- **Licence: MPL-2.0** (decided 2026-11-16), as most of gst-plugins-rs, its intended upstream home (see the
  language below). Using that licence from the first commit means no relicensing if it is adopted:
  - copyleft per file: changes to the plugin's own files stay open, and nothing else is affected (programs that load
    it, GStreamer, closed products using it);
  - compatible with Apache-2.0. SpatialAudioServer stays Apache-2.0: it only loads the plugin at run time, through
    GStreamer's plugin registry;
  - packages carry the licence text and point to the source (the public repository). The Windows MSI already meets
    the same duty for its embedded LGPL GStreamer.

## Language: Rust

*Decided 2026-11-16.*

| | C | Rust (chosen) |
|---|---|---|
| Build | Meson, from GStreamer's plugin template | `gstreamer-rs` with `cargo-c`, built as an ordinary plugin library |
| Fit | traditional; the sketches above are C | GStreamer's preferred language for new elements |
| Upstream home | gst-plugins-bad (then good) | gst-plugins-rs, an official module released with GStreamer and packaged by distributions |

The DSP is the same either way: cached coefficients per path, delay-buffer taps, multiply-adds. Rust because:
- a many-input, many-output engine is easier to keep memory-safe in it;
- gst-plugins-rs is probably the realistic way into GStreamer's releases;
- FFT: `rustfft` (MIT/Apache-2.0, SIMD on x86 and arm64) for partitioned convolution. The usual C choice, FFTW,
  is GPL and would not fit an MPL plugin.

The C sketches above stay as descriptions of the algorithms.

**Performance:** no meaningful cost compared with C.
- Same LLVM back end as clang, no runtime or garbage collector.
- Bounds checks are removed or cheap when loops run over slices and iterators (`chunks_exact`, `zip`) rather than
  indexing.
- SIMD through `std::arch` (SSE/AVX, NEON) or auto-vectorisation.
- The gstreamer-rs layer costs a little per buffer (reference counts, locks), not per sample.
- Precedent: gst-plugins-rs already ships real-time audio DSP, including `hrtfrender` (binaural rendering).

What matters more than the language, as in C:
- partitioned FFT convolution for long reverbs, not direct convolution;
- no allocation or contended locks in the per-buffer path;
- denormals flushed to zero (or a tiny offset added) in decaying tails.

Other costs: the plugin library is a few MB (Rust links statically), and builds are slower, so CI builds the plugin
only when its code changes (the Actions minutes budget in PACKAGING-PLAN.md).

**To check first:** whether gstreamer-rs supports subclassing `GstAudioAggregator` (the base class of `audiomixer`)
in the way the design needs. If not, the element builds on `GstAggregator` and handles audio formats itself: more
work, no slower.

## GStreamer versions

- GStreamer keeps binary compatibility within 1.x: a plugin built against an older 1.x runs on newer ones, though
  not the other way round.
- **Linux:** build against the oldest version SpatialAudioServer supports (1.24, see PACKAGING-PLAN.md). The result
  works on 1.24 through 1.28 and later.
- **Windows:** build with MSVC against the same GStreamer 1.28 that the SpatialAudioServer MSI embeds. The compiler
  must match the official MSVC builds of GStreamer.

## Packaging

The packaging follows [PACKAGING-PLAN.md](https://github.com/JakDaniels/SpatialAudioServer/blob/main/PACKAGING-PLAN.md): its own workflows, versions from tags, and draft
packages from its develop branch.

- **Linux:** its own `.deb` and `.rpm` (e.g. `gstreamer1.0-acousticspace`, Fedora `gstreamer1-acousticspace`) for x64
  and arm64:
  - installed into the system GStreamer plugin directory (`/usr/lib/<triplet>/gstreamer-1.0/`,
    `/usr/lib64/gstreamer-1.0/`);
  - depending on `gstreamer1.0-plugins-base` (≥ 1.24);
  - SpatialAudioServer's package *recommends* it, but does not require it.
- **Windows:** the plugin DLL goes into the SpatialAudioServer MSI's embedded GStreamer folder, beside the other
  plugins. No separate installer.
- **If it is adopted upstream:** distributions then ship it with GStreamer, and the separate package is retired: the
  next SpatialAudioServer package recommends the distribution's package instead.

## How SpatialAudioServer uses it

- **A new mix engine** (e.g. `acoustic`), chosen per room like `matrix` and `matrix-shared`. Engines are pluggable
  per room, and the engine suites already run once per engine, so it gets the same end-to-end tests.
- **Only when installed:** the engine is offered only when the `acousticspace` element exists. `/api/status` already
  reports missing elements. A server without the plugin keeps working with the existing engines.
- **Control:** properties on each request pad in place of the custom events proposed above:
  - per participant: `x`, `y`, `z`, `yaw` (and later `pitch`, `roll`), `gain`, `active`;
  - per element: the room preset (`environment`), and later geometry.

  Request-pad properties are GStreamer's usual way to control per-stream settings (as `audiomixer`'s `volume` and
  `mute` on its sink pads), they work with any binding, and the server already sets element properties of this
  kind (`audiomixmatrix`'s matrix). The element turns
  property changes into its coefficient updates; the audio path never sees them.
- **Positions:** the server's spatial policy already computes where each participant is and how it faces
  (`EffectivePose`), so the engine passes those on. The distance curve and the profiles (`outdoor`, `meeting`) map onto
  the element's distance model.

## Testing

- **In its own repository:** unit tests with GStreamer's check framework (`gst::check` in gstreamer-rs):
  - known inputs, checking gain, ITD and taps on the outputs;
  - pads added and removed while playing;
  - a listener's own input never in their own mix;
  - CPU per path measured, for the scaling claims above.
- **In SpatialAudioServer:** the engine suites with the plugin installed, and the load tool for real rooms.

## Getting it adopted

GStreamer takes new elements when they are generally useful, maintained and well tested. A multi-listener
acoustic-space mixer is specialised, so expect discussion. The way there:
1. working and tested out of tree;
2. documentation (element docs, examples with `gst-launch-1.0`);
3. ideally users besides SpatialAudioServer;
4. then a merge request to gst-plugins-rs on GStreamer's GitLab.
