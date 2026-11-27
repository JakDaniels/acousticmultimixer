// Copyright 2026 Simon Annetts/Jak Daniels
// SPDX-License-Identifier: MPL-2.0

//! The element's spatial mixing: known inputs in, exact left/right outputs checked against the formulas.

use gst::prelude::*;

use byte_slice_cast::*;

use crate::spatial::{DistanceParams, Orientation, render};

fn init() {
    super::acousticmultimixer::init();
}

const FRAMES: usize = 480; // 10 ms at 48 kHz, one output buffer

/// A pipeline with the element and one appsink per participant, fed by hand one buffer at a time.
struct Room {
    pipeline: gst::Pipeline,
    mixer: gst::Element,
    sinks: Vec<gst::Pad>,
    appsinks: Vec<Option<gst_app::AppSink>>,
    k: u64,
}

impl Drop for Room {
    fn drop(&mut self) {
        let _ = self.pipeline.set_state(gst::State::Null);
    }
}

impl Room {
    /// `listening[n]`: participant n's output is linked.
    fn new(listening: &[bool], configure: impl FnOnce(&gst::Element)) -> Room {
        init();
        let pipeline = gst::Pipeline::default();
        let mixer = gst::ElementFactory::make("acousticmultimixer")
            .property("output-buffer-duration", 10.mseconds())
            .build()
            .unwrap();
        // The exact values below are the stereo pan's (SpatialAudioServer's matrix); the head model has its own
        // tests.
        mixer.set_property_from_str("direction-model", "pan");
        mixer.set_property("air-absorption", 0.0);
        configure(&mixer);
        pipeline.add(&mixer).unwrap();

        let caps = gst_audio::AudioInfo::builder(gst_audio::AUDIO_FORMAT_F32, 48_000, 1)
            .build()
            .unwrap()
            .to_caps()
            .unwrap();
        let mut sinks = vec![];
        let mut appsinks = vec![];
        for (n, &linked) in listening.iter().enumerate() {
            let sink = mixer.request_pad_simple(&format!("sink_{n}")).unwrap();
            sinks.push(sink);
            if linked {
                let appsink = gst_app::AppSink::builder()
                    .caps(
                        &gst::Caps::builder("audio/x-raw")
                            .field("format", "F32LE")
                            .build(),
                    )
                    .async_(false)
                    .sync(false)
                    .build();
                pipeline.add(&appsink).unwrap();
                mixer
                    .link_pads(Some(&format!("mix_{n}")), &appsink, None)
                    .unwrap();
                appsinks.push(Some(appsink));
            } else {
                appsinks.push(None);
            }
        }
        pipeline.set_state(gst::State::Playing).unwrap();

        let segment = gst::FormattedSegment::<gst::ClockTime>::new();
        for (n, sink) in sinks.iter().enumerate() {
            assert!(sink.send_event(gst::event::StreamStart::builder(&format!("s{n}")).build()));
            assert!(sink.send_event(gst::event::Caps::builder(&caps).build()));
            assert!(sink.send_event(gst::event::Segment::builder(&segment).build()));
        }

        Room {
            pipeline,
            mixer,
            sinks,
            appsinks,
            k: 0,
        }
    }

    fn pad(&self, n: usize) -> &gst::Pad {
        &self.sinks[n]
    }

    /// One buffer of a constant per input; returns each linked output's interleaved stereo samples.
    fn step(&mut self, values: &[f32]) -> Vec<Option<Vec<f32>>> {
        for (sink, &v) in self.sinks.iter().zip(values) {
            let mut buffer = gst::Buffer::with_size(FRAMES * 4).unwrap();
            {
                let b = buffer.get_mut().unwrap();
                b.set_pts(gst::ClockTime::from_mseconds(10 * self.k));
                b.set_duration(gst::ClockTime::from_mseconds(10));
                b.map_writable()
                    .unwrap()
                    .as_mut_slice_of::<f32>()
                    .unwrap()
                    .fill(v);
            }
            sink.chain(buffer).unwrap();
        }
        let out = self
            .appsinks
            .iter()
            .map(|a| {
                a.as_ref().map(|a| {
                    let sample = a
                        .try_pull_sample(gst::ClockTime::from_seconds(5))
                        .expect("output");
                    let buffer = sample.buffer().unwrap();
                    assert_eq!(
                        buffer.pts(),
                        Some(gst::ClockTime::from_mseconds(10 * self.k))
                    );
                    let map = buffer.map_readable().unwrap();
                    let v = map.as_slice_of::<f32>().unwrap().to_vec();
                    assert_eq!(v.len(), 2 * FRAMES);
                    v
                })
            })
            .collect();
        self.k += 1;
        out
    }
}

/// Every frame of `out` is (l, r).
fn assert_constant(out: &[f32], (l, r): (f64, f64), what: &str) {
    for (i, f) in out.as_chunks::<2>().0.iter().enumerate() {
        assert!(
            (f[0] as f64 - l).abs() < 1e-4 && (f[1] as f64 - r).abs() < 1e-4,
            "{what}: frame {i} = ({}, {}), expected ({l}, {r})",
            f[0],
            f[1]
        );
    }
}

fn place(pad: &gst::Pad, x: f64, y: f64, yaw: f64) {
    pad.set_property("x", x);
    pad.set_property("y", y);
    pad.set_property("yaw", yaw);
}

/// What `listener` hears of `speakers` (value, position) by the formulas.
fn expected(speakers: &[(f32, [f64; 3])], listener: [f64; 3], yaw: f64) -> (f64, f64) {
    let orientation = Orientation {
        yaw,
        ..Orientation::default()
    };
    speakers.iter().fold((0.0, 0.0), |(l, r), &(v, at)| {
        let (gl, gr) = render(
            Some(at),
            Some((listener, orientation)),
            &DistanceParams::default(),
        )
        .stereo();
        (l + gl * v as f64, r + gr * v as f64)
    })
}

#[test]
fn positions_give_level_and_pan() {
    let mut room = Room::new(&[true, true, true], |_| {});
    // 0 at the origin facing +x; 1 four metres to 0's left; 2 eight metres ahead of 0, facing back (−x).
    place(room.pad(0), 0.0, 0.0, 0.0);
    place(room.pad(1), 0.0, 4.0, 0.0);
    place(room.pad(2), 8.0, 0.0, 180.0);
    let v = [1.0, 10.0, 100.0];
    let at = [[0.0, 0.0, 0.0], [0.0, 4.0, 0.0], [8.0, 0.0, 0.0]];

    let out = room.step(&v);
    let e0 = expected(&[(v[1], at[1]), (v[2], at[2])], at[0], 0.0);
    // Sanity check against the numbers by hand: 1 is hard left at −6 dB, 2 centred at −12 dB.
    assert!((e0.0 - (10.0 * 0.5 + 100.0 * 0.25)).abs() < 1e-9);
    assert!((e0.1 - 100.0 * 0.25).abs() < 1e-9);
    assert_constant(out[0].as_ref().unwrap(), e0, "mix_0");
    assert_constant(
        out[1].as_ref().unwrap(),
        expected(&[(v[0], at[0]), (v[2], at[2])], at[1], 0.0),
        "mix_1",
    );
    assert_constant(
        out[2].as_ref().unwrap(),
        expected(&[(v[0], at[0]), (v[1], at[1])], at[2], 180.0),
        "mix_2",
    );
}

#[test]
fn moving_ramps_across_one_buffer() {
    let mut room = Room::new(&[true, true], |_| {});
    place(room.pad(0), 0.0, 0.0, 0.0);
    place(room.pad(1), 0.0, 4.0, 0.0); // hard left, 0.5
    let out = room.step(&[0.0, 1.0]);
    assert_constant(out[0].as_ref().unwrap(), (0.5, 0.0), "before");

    // Now hard right at 2 m: (0, 1). The next buffer ramps, frame k at (k + 1) / FRAMES of the way.
    room.pad(1).set_property("y", -2.0);
    let out = room.step(&[0.0, 1.0]);
    let out = out[0].as_ref().unwrap();
    for (k, f) in out.as_chunks::<2>().0.iter().enumerate() {
        let t = (k + 1) as f32 / FRAMES as f32;
        let (l, r) = (0.5 * (1.0 - t), t);
        assert!(
            (f[0] - l).abs() < 1e-5 && (f[1] - r).abs() < 1e-5,
            "frame {k}: ({}, {}), expected ({l}, {r})",
            f[0],
            f[1]
        );
    }
    let out = room.step(&[0.0, 1.0]);
    assert_constant(out[0].as_ref().unwrap(), (0.0, 1.0), "after");
}

#[test]
fn gain_mute_and_trims() {
    let mut room = Room::new(&[true, true, true], |_| {});
    let v = [1.0, 10.0, 100.0];
    let out = room.step(&v);
    assert_constant(out[0].as_ref().unwrap(), (110.0, 110.0), "unpositioned");

    room.pad(1).set_property("gain", 2.0);
    room.pad(2).set_property("mute", true);
    // 1 turns 0 down by half, for themselves only.
    room.pad(1).set_property(
        "trims",
        "trims,sink_0=0.5".parse::<gst::Structure>().unwrap(),
    );
    room.step(&v); // the ramp
    let out = room.step(&v);
    assert_constant(out[0].as_ref().unwrap(), (20.0, 20.0), "mix_0");
    assert_constant(out[1].as_ref().unwrap(), (0.5, 0.5), "mix_1");
    assert_constant(out[2].as_ref().unwrap(), (21.0, 21.0), "mix_2");

    let trims = room.pad(1).property::<gst::Structure>("trims");
    assert_eq!(trims.get::<f64>("sink_0").unwrap(), 0.5);
}

#[test]
fn separate_listener_pose() {
    let mut room = Room::new(&[true, true], |_| {});
    place(room.pad(0), 0.0, 0.0, 0.0);
    place(room.pad(1), 0.0, 4.0, 0.0);
    // 0 speaks from the origin but listens from just south of 1, facing north: 1 is ahead, 1 m away.
    room.pad(0).set_property("listener-y", 3.0);
    room.pad(0).set_property("listener-yaw", 90.0);
    assert!(room.pad(0).property::<bool>("listener-separate"));
    let out = room.step(&[1.0, 1.0]);
    assert_constant(out[0].as_ref().unwrap(), (1.0, 1.0), "mix_0");
    // 1 still hears 0 from the origin: 4 m away, to the right of someone facing +x.
    assert_constant(out[1].as_ref().unwrap(), (0.0, 0.5), "mix_1");
}

#[test]
fn distance_model_and_range() {
    let mut room = Room::new(&[true, true], |m| {
        m.set_property_from_str("distance-model", "linear");
        m.set_property("reference-distance", 10.0);
        m.set_property("rolloff", 2.0);
    });
    place(room.pad(0), 0.0, 0.0, 0.0);
    place(room.pad(1), 22.5, 0.0, 0.0); // halfway down the meeting curve
    let out = room.step(&[0.0, 1.0]);
    assert_constant(out[0].as_ref().unwrap(), (0.5, 0.5), "linear");

    room.mixer.set_property("max-range", 20.0);
    room.step(&[0.0, 1.0]);
    let out = room.step(&[0.0, 1.0]);
    assert_constant(out[0].as_ref().unwrap(), (0.0, 0.0), "out of range");
}

#[test]
fn alone_is_silent_and_talk_only_is_not_mixed() {
    let mut room = Room::new(&[true], |_| {});
    let out = room.step(&[1.0]);
    assert_constant(out[0].as_ref().unwrap(), (0.0, 0.0), "alone");
    drop(room);

    // 1 only talks (mix_1 unlinked); 0 still hears them.
    let mut room = Room::new(&[true, false], |_| {});
    let out = room.step(&[1.0, 10.0]);
    assert_constant(out[0].as_ref().unwrap(), (10.0, 10.0), "mix_0");
    assert!(out[1].is_none());
}

#[test]
fn pad_properties_from_launch_syntax() {
    init();
    let pipeline = gst::parse::launch(
        "acousticmultimixer name=m sink_0::x=3.5 sink_1::trims=\"trims,sink_0=0.25\" \
           audiotestsrc num-buffers=1 ! m.sink_0 \
           audiotestsrc num-buffers=1 ! m.sink_1 \
           m.mix_0 ! queue ! fakesink m.mix_1 ! queue ! fakesink",
    )
    .unwrap()
    .downcast::<gst::Bin>()
    .unwrap();
    let m = pipeline.by_name("m").unwrap();
    let p0 = m.static_pad("sink_0").unwrap();
    assert_eq!(p0.property::<f64>("x"), 3.5);
    assert!(p0.property::<bool>("positioned"));
    let trims = m
        .static_pad("sink_1")
        .unwrap()
        .property::<gst::Structure>("trims");
    assert_eq!(trims.get::<f64>("sink_0").unwrap(), 0.25);
}

#[test]
fn head_model_delays_and_quietens_the_far_ear() {
    let mut room = Room::new(&[true, true], |m| {
        m.set_property_from_str("direction-model", "head");
    });
    place(room.pad(0), 0.0, 0.0, 0.0);
    place(room.pad(1), 0.0, -2.0, 0.0); // 2 m to 0's right: full level, hard right

    // From silence, a constant: the right ear has it at once; the left ear 0.66 ms (31.5 samples) later, then
    // rising through the head-shadow low-pass.
    let out = room.step(&[0.0, 1.0]);
    let frames: Vec<_> = out[0].as_ref().unwrap().as_chunks::<2>().0.to_vec();
    assert!(
        frames.iter().all(|f| (f[1] - 1.0).abs() < 1e-6),
        "right ear"
    );
    assert!(
        frames[..31].iter().all(|f| f[0] == 0.0),
        "left ear before the ITD"
    );
    assert!(frames[33][0] > 0.0, "left ear after the ITD");

    // Settled: the left ear is 6 dB down (a low-pass passes a constant unchanged).
    room.step(&[0.0, 1.0]);
    let out = room.step(&[0.0, 1.0]);
    let expected = 10f64.powf(-6.0 / 20.0);
    assert_constant(out[0].as_ref().unwrap(), (expected, 1.0), "settled");
}

#[test]
fn room_adds_early_reflections() {
    let mut room = Room::new(&[true, true], |m| {
        m.set_property_from_str("room", "small-room");
        // The reflections alone (a constant would set the reverb ringing for seconds).
        m.set_property("reverb-level", -60.0);
    });
    place(room.pad(0), 0.0, 0.0, 0.0);
    place(room.pad(1), 2.0, 0.0, 0.0); // 2 m ahead: full level, centred

    // From silence, a constant: the direct sound at once, the first reflection 3 ms later (2.76 ms in the left
    // ear, 3 ms in the right: its diffuse copies are moved apart).
    let out = room.step(&[0.0, 1.0]);
    let frames: Vec<_> = out[0].as_ref().unwrap().as_chunks::<2>().0.to_vec();
    assert!(
        frames[..132].iter().all(|f| f == &[1.0, 1.0]),
        "direct only"
    );
    assert!(frames[132][0] > 1.0 && frames[132][1] == 1.0, "left first");
    assert!(frames[144][1] > 1.0, "then right");

    // Settled (the walls' low-pass passes a constant): every tap adds its level in both ears.
    room.step(&[0.0, 1.0]);
    let out = room.step(&[0.0, 1.0]);
    let taps: f64 = [-9.0f64, -12.0, -15.0, -19.0]
        .iter()
        .map(|db| 10f64.powf(db / 20.0))
        .sum();
    assert_constant(
        out[0].as_ref().unwrap(),
        (1.0 + taps, 1.0 + taps),
        "settled",
    );

    // Outdoors again: the direct sound only.
    room.mixer.set_property_from_str("room", "outdoor");
    room.step(&[0.0, 1.0]);
    let out = room.step(&[0.0, 1.0]);
    assert_constant(out[0].as_ref().unwrap(), (1.0, 1.0), "outdoor");
}

#[test]
fn room_adds_a_reverb_tail_but_not_your_own() {
    let mut room = Room::new(&[true, true], |m| {
        m.set_property_from_str("room", "small-room");
    });
    place(room.pad(0), 0.0, 0.0, 0.0);
    place(room.pad(1), 2.0, 0.0, 0.0);
    let energy = |out: &[f32]| out.iter().map(|&v| (v as f64).powi(2)).sum::<f64>();

    // 10 ms of sound from participant 1, then silence. The reflections are over by 40 ms (the last tap is 17 ms
    // late); the tail goes on, in both ears, and participant 1 never hears it.
    let mut tail = [0.0; 2];
    for k in 0..30 {
        let out = room.step(&[0.0, if k == 0 { 1.0 } else { 0.0 }]);
        assert!(
            out[1].as_ref().unwrap().iter().all(|&v| v == 0.0),
            "own voice"
        );
        if k >= 4 {
            for frame in out[0].as_ref().unwrap().as_chunks::<2>().0 {
                tail[0] += (frame[0] as f64).powi(2);
                tail[1] += (frame[1] as f64).powi(2);
            }
        }
    }
    assert!(tail[0] > 1e-3 && tail[1] > 1e-3, "tail {tail:?}");

    // Turned off: nothing (audible) after the reflections.
    room.mixer.set_property("reverb-level", -60.0);
    for k in 0..10 {
        let out = room.step(&[0.0, if k == 0 { 1.0 } else { 0.0 }]);
        if k >= 4 {
            assert!(energy(out[0].as_ref().unwrap()) < 1e-20, "no tail");
        }
    }
}

#[test]
fn room_overrides_replace_the_presets_values() {
    let mut room = Room::new(&[true, true], |m| {
        m.set_property_from_str("room", "small-room");
        m.set_property("reverb-level", -60.0);
        // Two reflections instead of the preset's four, the levels written as integers.
        m.set_property_from_str("reflection-delays", "<5.0, 10.0>");
        m.set_property_from_str("reflection-levels", "<-6, -12>");
    });
    place(room.pad(0), 0.0, 0.0, 0.0);
    place(room.pad(1), 2.0, 0.0, 0.0);

    // The first arrives 5 ms late (4.6 ms in the left ear).
    let out = room.step(&[0.0, 1.0]);
    let frames: Vec<_> = out[0].as_ref().unwrap().as_chunks::<2>().0.to_vec();
    assert!(
        frames[..221].iter().all(|f| f == &[1.0, 1.0]),
        "direct only"
    );
    assert!(frames[221][0] > 1.0, "first reflection");
    room.step(&[0.0, 1.0]);
    let out = room.step(&[0.0, 1.0]);
    let taps = 10f64.powf(-6.0 / 20.0) + 10f64.powf(-12.0 / 20.0);
    assert_constant(
        out[0].as_ref().unwrap(),
        (1.0 + taps, 1.0 + taps),
        "two taps",
    );

    let p = room.mixer.property::<gst::Structure>("room-parameters");
    let list = |name: &str| -> Vec<f64> {
        p.get::<gst::Array>(name)
            .unwrap()
            .iter()
            .map(|v| v.get::<f64>().unwrap())
            .collect()
    };
    assert_eq!(list("reflection-delays"), [5.0, 10.0]);
    assert_eq!(list("reflection-levels"), [-6.0, -12.0]);
    assert_eq!(p.get::<f64>("wall-cutoff").unwrap(), 6000.0);
    assert_eq!(p.get::<f64>("reverb-decay").unwrap(), 0.4);
    let overridden: Vec<String> = p
        .get::<gst::Array>("overridden")
        .unwrap()
        .iter()
        .map(|v| v.get::<String>().unwrap())
        .collect();
    assert_eq!(overridden, ["reflection-delays", "reflection-levels"]);
    assert_eq!(room.mixer.property::<f64>("reverb-decay"), -1.0);

    // Back to the preset's: empty lists.
    room.mixer
        .set_property("reflection-delays", gst::Array::new(Vec::<f64>::new()));
    room.mixer
        .set_property("reflection-levels", gst::Array::new(Vec::<f64>::new()));
    for _ in 0..3 {
        room.step(&[0.0, 1.0]);
    }
    let out = room.step(&[0.0, 1.0]);
    let taps: f64 = [-9.0f64, -12.0, -15.0, -19.0]
        .iter()
        .map(|db| 10f64.powf(db / 20.0))
        .sum();
    assert_constant(out[0].as_ref().unwrap(), (1.0 + taps, 1.0 + taps), "preset");
}

#[test]
fn tuning_a_room_keeps_what_is_sounding() {
    let mut room = Room::new(&[true, true], |m| {
        m.set_property_from_str("room", "small-room");
        m.set_property("reverb-level", -60.0);
    });
    place(room.pad(0), 0.0, 0.0, 0.0);
    place(room.pad(1), 2.0, 0.0, 0.0);
    for _ in 0..3 {
        room.step(&[0.0, 1.0]);
    }
    // Shorter reflections, the same levels: the next buffer is already settled, as the ring kept its input.
    room.mixer
        .set_property_from_str("reflection-delays", "<1.0, 2.0, 3.0, 4.0>");
    let out = room.step(&[0.0, 1.0]);
    let taps: f64 = [-9.0f64, -12.0, -15.0, -19.0]
        .iter()
        .map(|db| 10f64.powf(db / 20.0))
        .sum();
    assert_constant(out[0].as_ref().unwrap(), (1.0 + taps, 1.0 + taps), "kept");

    // The reverb: a burst, then its tail carries on through a new decay time and a new size.
    room.mixer.set_property("reverb-level", 0.0);
    let energy = |out: &[f32]| out.iter().map(|&v| (v as f64).powi(2)).sum::<f64>();
    for k in 0..6 {
        room.step(&[0.0, if k == 0 { 1.0 } else { 0.0 }]);
    }
    room.mixer.set_property("reverb-decay", 1.0);
    let out = room.step(&[0.0, 0.0]);
    assert!(energy(out[0].as_ref().unwrap()) > 1e-6, "tail after decay");
    room.mixer.set_property("reverb-size", 30.0);
    let out = room.step(&[0.0, 0.0]);
    assert!(energy(out[0].as_ref().unwrap()) > 1e-6, "tail after size");
    let p = room.mixer.property::<gst::Structure>("room-parameters");
    assert_eq!(p.get::<f64>("reverb-size").unwrap(), 30.0);
}
