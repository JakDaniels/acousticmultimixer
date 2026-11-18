# Benchmarks

Measured with `scripts/bench.py` (release build). Each figure is CPU time (user + system) per second of audio, as
a percentage of one core, with the test sources' own cost (`audiotestsrc`, measured alone) subtracted. Neither
side includes the Opus encoders or WebRTC, which cost the same either way.

- **acousticmultimixer:** one element, a `queue` and `fakesink` per output.
- **matrix:** SpatialAudioServer's `matrix` engine's mixing: `audiointerleave` → `tee` → per listener
  `queue` → `audiomixmatrix` (2 × N gains, 1e-9 for "off") → `fakesink`.
- Participants on a 5 m grid with different facings (gains and pans all differ), outdoor distance model.
- **Quiet:** a sine at −80 dB, as decoded comfort noise: real audio to mix. **Silent (GAP):** buffers flagged GAP,
  which aggregators skip.

## 2026-11-18: step 3 (head model and air absorption)

Same machine. As fast as possible, 32 participants, 30 s, acousticmultimixer only (`--ours-only --props …`):

| case | `pan`, no air | `pan` (air on) | `head` (first version) | `head` (final) |
|---|---|---|---|---|
| 32 talking | 24.3 % | 101.2 % | 102.7 % | 44.4 % |
| 3 talking, 29 quiet | 23.6 % | 103.1 % | 101.5 % | 47.1 % |
| 3 talking, 29 silent (GAP) | 18.0 % | 26.3 % | 26.0 % | 21.6 % |

Any delay or filter on a path takes it off the plain-gain loop, so `pan` with air absorption costs as much as
`head`. The final version loops across each frame's ears with the delays' offsets worked out once per buffer.

Live, 32 participants, 20 s, defaults (`head`, air 0.1):

| case | sources alone | acousticmultimixer | matrix |
|---|---|---|---|
| 32 talking | 13.4 % | 51.5 % | 28.3 % |
| 3 talking, 29 quiet | 12.8 % | 47.1 % | 30.7 % |
| 3 talking, 29 silent (GAP) | 9.6 % | 22.3 % | 27.0 % |

## 2026-11-18: step 2 (stereo, distance and pan)

Machine: Intel Pentium Silver N6005 (4 cores, 2.0 GHz), Ubuntu 26.04, GStreamer 1.28.2, Rust 1.100.

32 participants, live (real time, 20 s):

| case | sources alone | acousticmultimixer | matrix |
|---|---|---|---|
| 32 talking | 13.3 % | 21.0 % | 28.5 % |
| 3 talking, 29 quiet | 14.1 % | 20.8 % | 30.5 % |
| 3 talking, 29 silent (GAP) | 10.1 % | 22.0 % | 25.7 % |

32 participants, as fast as possible (60 s of audio):

| case | sources alone | acousticmultimixer | matrix |
|---|---|---|---|
| 32 talking | 6.8 % | 23.5 % | 30.2 % |
| 3 talking, 29 quiet | 5.8 % | 24.7 % | 31.0 % |
| 3 talking, 29 silent (GAP) | 1.6 % | 18.3 % | 29.1 % |

Where the time goes (as fast as possible, 3 talking, 29 GAP, sources subtracted):

| pipeline | CPU |
|---|---|
| plain `audiomixer`, 32 inputs, one output | 7.1 % |
| acousticmultimixer, one output linked | 9.3 % |
| acousticmultimixer, 32 outputs, no queues (`fakesink async=false`) | 11.3 % |
| acousticmultimixer, 32 outputs, a queue each | 18.3 % |

The mixing itself is now a small part: the cost barely changes between 3 and 32 talking. Most of it is GStreamer's
work per buffer: the aggregator taking 32 inputs (as much as a plain `audiomixer`), and pushing 32 outputs through
their queues (which every per-listener design has; SpatialAudioServer has one per encoder anyway).

## 2026-11-18: step 1 (the template)

10 minutes of audio, 3 participants, as fast as possible, sources included: renegotiating (option a) 7.6 s of
user CPU; fixed width (option b) at width 3: 7.7 s, 32: 9.6 s, 256: 25.4 s. The template's inner loop visited
every output channel for every sample; step 2's loop visits only the listeners who hear each input.
