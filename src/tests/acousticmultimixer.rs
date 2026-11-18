// GStreamer acousticmultimixer element - unit tests
//
// Copyright (C) 2026 Taruntej Kanakamalla <tarun centricular com>
// Copyright (C) 2026 Tim-Philipp Müller <tim centricular com>
// Copyright 2026 Simon Annetts/Jak Daniels
//
// Taken from gst-plugins-rs audio/audiomultimixer (commit 916404886,
// 2026-09-11), with changes for acousticmultimixer.
//
// This Source Code Form is subject to the terms of the Mozilla Public License, v2.0.
// If a copy of the MPL was not distributed with this file, You can obtain one at
// <https://mozilla.org/MPL/2.0/>.
//
// SPDX-License-Identifier: MPL-2.0

use gst::prelude::*;

use std::collections::HashMap;
use std::sync::Once;

use byte_slice_cast::*;

fn init() {
    static INIT: Once = Once::new();

    INIT.call_once(|| {
        gst::init().unwrap();
        crate::plugin_register_static().expect("plugin registration");
    });
}

struct Pipeline(gst::Pipeline);

impl Drop for Pipeline {
    fn drop(&mut self) {
        let _ = self.0.set_state(gst::State::Null);
    }
}

impl std::ops::Deref for Pipeline {
    type Target = gst::Pipeline;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[test]
fn mixer_basic() {
    init();

    let rate = 48_000;
    let bands = 512;
    let band_width = rate / 2 / bands;
    let threshold = -80.0;
    let input_freq = [440, 880, 1760];
    let spectra = HashMap::from([("s0", [880, 1760]), ("s1", [440, 1760]), ("s2", [440, 880])]);

    let pipeline = Pipeline(gst::parse::launch(&format!(
        "acousticmultimixer name=mixer sample-rate={rate}
            audiotestsrc num-buffers=10 samplesperbuffer=960 freq={f0} ! mixer.sink_0
            audiotestsrc num-buffers=10 samplesperbuffer=960 freq={f1} ! mixer.sink_1
            audiotestsrc num-buffers=10 samplesperbuffer=960 freq={f2} ! mixer.sink_2
            mixer.mix_0 ! queue ! spectrum name=s0 interval=200000000 post-messages=true bands={bands} ! fakesink
            mixer.mix_1 ! queue ! spectrum name=s1 interval=200000000 post-messages=true bands={bands} ! fakesink
            mixer.mix_2 ! queue ! spectrum name=s2 interval=200000000 post-messages=true bands={bands} ! fakesink
            ",
        f0 = input_freq[0],
        f1 = input_freq[1],
        f2 = input_freq[2],
    ))
    .unwrap()
    .downcast::<gst::Pipeline>()
    .unwrap());

    pipeline.debug_to_dot_file(gst::DebugGraphDetails::empty(), "pipeline.dot");

    pipeline.set_state(gst::State::Playing).unwrap();

    let mut eos = false;
    let bus = pipeline.bus().unwrap();
    while let Some(msg) = bus.timed_pop(gst::ClockTime::NONE) {
        use gst::MessageView;
        match msg.view() {
            MessageView::Eos(..) => {
                eos = true;
                break;
            }
            MessageView::Error(e) => panic!("Received error on bus {e}"),
            MessageView::Element(_) => {
                let s = msg.structure().unwrap();
                println!(" {}", s.name());

                let Some(ele) = msg.src() else {
                    break;
                };

                println!("{}", ele.name());
                let Some(freq) = spectra.get(ele.name().as_str()) else {
                    break;
                };

                let mags = s
                    .get::<gst::List>("magnitude")
                    .unwrap()
                    .iter()
                    .map(|v| v.get::<f32>().unwrap())
                    .collect::<Vec<f32>>();

                assert_eq!(mags.len(), bands);

                let mut peak_mag = threshold;
                let mut peak_count = 0_usize;
                for (bin, mag) in mags.iter().enumerate() {
                    // let f = bin * 44100 / (bands * 2);
                    // println!("{mag}: {f}");

                    peak_mag = f32::max(*mag, peak_mag);
                    if (peak_mag > *mag) && (peak_mag > -25.0) {
                        //previous magnitude was the peak, get its frequency
                        let peak_freq = bin * band_width - band_width / 2;

                        println!("peak_mag: {peak_mag} {peak_freq} ");
                        println!("expected {freq:?} {peak_count}");

                        // ~20Hz of tolerance
                        assert!(
                            (peak_freq.saturating_sub(band_width)
                                ..peak_freq.saturating_add(band_width))
                                .contains(&freq[peak_count])
                        );

                        // TODO handle if frequency is in the same range for two or more streams?
                        peak_count += 1;

                        // reset the peak to threshold
                        peak_mag = threshold;
                    }
                }
            }
            _ => (),
        }
    }

    pipeline.set_state(gst::State::Null).unwrap();

    assert!(eos);
}

fn mixer_direct_link<
    T: byte_slice_cast::FromByteSlice
        + std::fmt::Display
        + std::ops::Add
        + std::ops::Add<Output = T>
        + std::ops::Div
        + std::ops::Div<Output = T>
        + std::ops::Sub
        + std::ops::Sub<Output = T>
        + Copy,
>(
    format: gst_audio::AudioFormat,
    values: &[T; 3],
    check_func: fn(T, T) -> bool,
) where
    <T as std::ops::Add>::Output: std::ops::Div<T>,
{
    init();

    let pipeline = Pipeline(gst::Pipeline::default());

    let mixer = gst::ElementFactory::make("acousticmultimixer")
        .property("output-buffer-duration", 10.mseconds())
        .property("max-participants", 33u32)
        .build()
        .unwrap();

    let caps = gst::Caps::builder("audio/x-raw")
        .field("format", format.to_str())
        .build();

    let appsink_3 = gst_app::AppSink::builder()
        .name("appsink_3")
        .property("caps", &caps)
        .async_(false)
        .sync(false)
        .build();
    let appsink_17 = gst_app::AppSink::builder()
        .name("appsink_17")
        .property("caps", &caps)
        .async_(false)
        .sync(false)
        .build();
    let appsink_32 = gst_app::AppSink::builder()
        .name("appsink_32")
        .property("caps", caps)
        .async_(false)
        .sync(false)
        .build();

    pipeline.add(&mixer).unwrap();
    pipeline.add(&appsink_3).unwrap();
    pipeline.add(&appsink_17).unwrap();
    pipeline.add(&appsink_32).unwrap();

    let sink_3 = mixer.request_pad_simple("sink_3").expect("request sink_3");
    let sink_17 = mixer
        .request_pad_simple("sink_17")
        .expect("request sink_17");
    let sink_32 = mixer
        .request_pad_simple("sink_32")
        .expect("request sink_32");

    mixer.link_pads(Some("mix_3"), &appsink_3, None).unwrap();
    mixer.link_pads(Some("mix_17"), &appsink_17, None).unwrap();
    mixer.link_pads(Some("mix_32"), &appsink_32, None).unwrap();

    pipeline.set_state(gst::State::Playing).unwrap();

    let segment = gst::FormattedSegment::<gst::ClockTime>::new();
    let caps = gst_audio::AudioInfo::builder(format, 48_000, 1)
        .build()
        .unwrap()
        .to_caps()
        .unwrap();

    for (i, sink) in [&sink_3, &sink_17, &sink_32].into_iter().enumerate() {
        assert!(sink.send_event(gst::event::StreamStart::builder(&format!("stream-{i}")).build()));
        assert!(sink.send_event(gst::event::Caps::builder(&caps).build()));
        assert!(sink.send_event(gst::event::Segment::builder(&segment).build()));
    }

    for (i, sink) in [&sink_3, &sink_17, &sink_32].into_iter().enumerate() {
        let mut buffer = gst::Buffer::with_size(480 * std::mem::size_of::<T>()).unwrap();
        {
            let buffer = buffer.get_mut().unwrap();
            buffer.set_pts(gst::ClockTime::ZERO);
            let mut map = buffer.map_writable().unwrap();
            let slice = map.as_mut_slice_of::<T>().unwrap();

            slice.fill(values[i]);
        }
        sink.chain(buffer).unwrap();
    }

    for (i, sink) in [&appsink_3, &appsink_17, &appsink_32]
        .into_iter()
        .enumerate()
    {
        println!("pulling {i}");
        let sample = sink.pull_sample().unwrap();
        let buffer = sample.buffer().unwrap();
        let map = buffer.map_readable().unwrap();
        let slice = map.as_slice_of::<T>().unwrap();
        assert_eq!(slice.len(), 480);

        let expected: T = match i {
            0 => values[1] + values[2],
            1 => values[0] + values[2],
            2 => values[0] + values[1],
            _ => unreachable!(),
        };
        for (j, &sample) in slice.iter().enumerate() {
            assert!(
                check_func(sample, expected),
                "{} sample[{j}] = {sample}, expected {expected}",
                sink.name(),
            )
        }
    }

    println!("shutdown");

    pipeline.set_state(gst::State::Null).unwrap();
}

#[test]
fn mixer_direct_link_f32() {
    mixer_direct_link::<f32>(
        gst_audio::AUDIO_FORMAT_F32,
        &[1.0, 10.0, 100.0],
        |sample, expected| (sample - expected).abs() < 1e-6,
    );
}

#[test]
fn mixer_direct_link_s16() {
    mixer_direct_link::<i16>(
        gst_audio::AUDIO_FORMAT_S16,
        &[1, 10, 100],
        |sample, expected| sample == expected,
    );
}

// Inputs added and removed while running: the mixer's output width changes, so it renegotiates (the code that
// needs the AudioAggregator workarounds). Every output must carry the sum of the other inputs after each change.
#[test]
fn mixer_pads_added_and_removed_while_running() {
    init();

    let pipeline = Pipeline(gst::Pipeline::default());

    let mixer = gst::ElementFactory::make("acousticmultimixer")
        .property("output-buffer-duration", 10.mseconds())
        .build()
        .unwrap();
    pipeline.add(&mixer).unwrap();

    let format = gst_audio::AUDIO_FORMAT_F32;
    let caps = gst_audio::AudioInfo::builder(format, 48_000, 1)
        .build()
        .unwrap()
        .to_caps()
        .unwrap();
    let segment = gst::FormattedSegment::<gst::ClockTime>::new();

    // Participant number -> (sink pad, appsink, value it sends)
    let mut participants: Vec<(u32, gst::Pad, gst_app::AppSink, f32)> = vec![];

    let add = |participants: &mut Vec<(u32, gst::Pad, gst_app::AppSink, f32)>, n: u32| {
        let appsink = gst_app::AppSink::builder()
            .name(format!("appsink_{n}"))
            .property(
                "caps",
                gst::Caps::builder("audio/x-raw")
                    .field("format", format.to_str())
                    .build(),
            )
            .async_(false)
            .sync(false)
            .build();
        pipeline.add(&appsink).unwrap();
        appsink.sync_state_with_parent().unwrap();

        let sink = mixer
            .request_pad_simple(&format!("sink_{n}"))
            .expect("request sink pad");
        mixer
            .link_pads(Some(&format!("mix_{n}")), &appsink, None)
            .unwrap();

        assert!(sink.send_event(gst::event::StreamStart::builder(&format!("stream-{n}")).build()));
        assert!(sink.send_event(gst::event::Caps::builder(&caps).build()));
        assert!(sink.send_event(gst::event::Segment::builder(&segment).build()));

        participants.push((n, sink, appsink, 10f32.powi(n as i32)));
    };

    let remove = |participants: &mut Vec<(u32, gst::Pad, gst_app::AppSink, f32)>, n: u32| {
        let i = participants.iter().position(|p| p.0 == n).unwrap();
        let (_, sink, appsink, _) = participants.remove(i);
        mixer.release_request_pad(&sink);
        appsink.set_state(gst::State::Null).unwrap();
        pipeline.remove(&appsink).unwrap();
    };

    let round = |participants: &[(u32, gst::Pad, gst_app::AppSink, f32)], k: u64| {
        for (_, sink, _, value) in participants {
            let mut buffer = gst::Buffer::with_size(480 * std::mem::size_of::<f32>()).unwrap();
            {
                let buffer = buffer.get_mut().unwrap();
                buffer.set_pts(gst::ClockTime::from_mseconds(10 * k));
                buffer.set_duration(gst::ClockTime::from_mseconds(10));
                let mut map = buffer.map_writable().unwrap();
                map.as_mut_slice_of::<f32>().unwrap().fill(*value);
            }
            sink.chain(buffer).unwrap();
        }

        let total: f32 = participants.iter().map(|p| p.3).sum();
        for (n, _, appsink, value) in participants {
            let sample = appsink
                .try_pull_sample(gst::ClockTime::from_seconds(5))
                .unwrap_or_else(|| panic!("round {k}: no output on mix_{n}"));
            let buffer = sample.buffer().unwrap();
            assert_eq!(buffer.pts(), Some(gst::ClockTime::from_mseconds(10 * k)));
            let map = buffer.map_readable().unwrap();
            let slice = map.as_slice_of::<f32>().unwrap();
            assert_eq!(slice.len(), 480, "round {k}: mix_{n} length");
            let expected = total - value;
            for (j, &s) in slice.iter().enumerate() {
                assert!(
                    (s - expected).abs() < 1e-3,
                    "round {k}: mix_{n} sample[{j}] = {s}, expected {expected}"
                );
            }
        }
    };

    add(&mut participants, 0);
    add(&mut participants, 1);
    pipeline.set_state(gst::State::Playing).unwrap();

    let mut k = 0;
    round(&participants, k);
    k += 1;

    // One joins, then another, then the first one leaves, then someone else, then a rejoin.
    for step in [
        (true, 2),
        (true, 3),
        (false, 0),
        (false, 2),
        (true, 4),
        (true, 0),
    ] {
        match step {
            (true, n) => add(&mut participants, n),
            (false, n) => remove(&mut participants, n),
        }
        for _ in 0..3 {
            round(&participants, k);
            k += 1;
        }
    }

    pipeline.set_state(gst::State::Null).unwrap();
}

// Live sources, with inputs joining and leaving every few milliseconds while audio flows, so changes land while
// the mixer is part way through an output buffer. Nothing may error, and every output still present at the end
// must still be receiving audio.
#[test]
fn mixer_live_churn() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};

    init();

    let pipeline = Pipeline(gst::Pipeline::default());
    let mixer = gst::ElementFactory::make("acousticmultimixer")
        .property("output-buffer-duration", 10.mseconds())
        .property("latency", 20.mseconds())
        .build()
        .unwrap();
    pipeline.add(&mixer).unwrap();

    struct Participant {
        n: u32,
        src: gst::Element,
        sink_pad: gst::Pad,
        fakesink: gst::Element,
        buffers: Arc<AtomicU64>,
    }

    // Asks for "sink_%u": the element picks the lowest free number, so numbers get reused.
    let add = || -> Participant {
        // 7 ms input buffers against 10 ms output buffers, so they rarely line up.
        let src = gst::ElementFactory::make("audiotestsrc")
            .property("is-live", true)
            .property("samplesperbuffer", 336i32)
            .property("freq", 300.0)
            .build()
            .unwrap();
        let fakesink = gst::ElementFactory::make("fakesink")
            .property("async", false)
            .property("sync", false)
            .property("signal-handoffs", true)
            .build()
            .unwrap();
        let buffers = Arc::new(AtomicU64::new(0));
        let counter = buffers.clone();
        fakesink.connect("handoff", false, move |_| {
            counter.fetch_add(1, Ordering::Relaxed);
            None
        });
        pipeline.add_many([&src, &fakesink]).unwrap();

        let sink_pad = mixer.request_pad_simple("sink_%u").unwrap();
        let n: u32 = sink_pad
            .name()
            .strip_prefix("sink_")
            .unwrap()
            .parse()
            .unwrap();
        src.static_pad("src").unwrap().link(&sink_pad).unwrap();
        mixer
            .link_pads(Some(&format!("mix_{n}")), &fakesink, None)
            .unwrap();
        fakesink.sync_state_with_parent().unwrap();
        src.sync_state_with_parent().unwrap();

        Participant {
            n,
            src,
            sink_pad,
            fakesink,
            buffers,
        }
    };

    let remove = |p: Participant| {
        p.src.set_state(gst::State::Null).unwrap();
        mixer.release_request_pad(&p.sink_pad);
        p.fakesink.set_state(gst::State::Null).unwrap();
        pipeline.remove_many([&p.src, &p.fakesink]).unwrap();
    };

    let mut participants: Vec<Participant> = (0..4).map(|_| add()).collect();
    pipeline.set_state(gst::State::Playing).unwrap();

    let bus = pipeline.bus().unwrap();
    let check_bus = || {
        while let Some(msg) = bus.pop() {
            if let gst::MessageView::Error(e) = msg.view() {
                panic!("error on bus: {e} ({:?})", e.debug());
            }
        }
    };

    // A small fixed pseudo-random sequence, so failures can be repeated.
    let mut seed = 12345u32;
    let mut next = |m: u32| {
        seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
        (seed >> 16) % m
    };

    for _ in 0..150 {
        std::thread::sleep(std::time::Duration::from_millis(3 + next(17) as u64));
        if participants.len() < 3 || (participants.len() < 12 && next(2) == 0) {
            participants.push(add());
        } else {
            let i = next(participants.len() as u32) as usize;
            remove(participants.remove(i));
        }
        check_bus();
    }

    // Everyone still here must keep getting audio.
    let before: Vec<u64> = participants
        .iter()
        .map(|p| p.buffers.load(Ordering::Relaxed))
        .collect();
    std::thread::sleep(std::time::Duration::from_millis(300));
    check_bus();
    for (p, b) in participants.iter().zip(before) {
        let after = p.buffers.load(Ordering::Relaxed);
        assert!(
            after >= b + 10,
            "mix_{} got {} buffers in 300 ms",
            p.n,
            after - b
        );
    }

    pipeline.set_state(gst::State::Null).unwrap();
}

// Pads are numbered below max-participants; "sink_%u" takes the lowest free number.
#[test]
fn mixer_capacity() {
    init();

    let mixer = gst::ElementFactory::make("acousticmultimixer")
        .property("max-participants", 3u32)
        .build()
        .unwrap();

    let names = |pads: &[gst::Pad]| {
        pads.iter()
            .map(|p| p.name().to_string())
            .collect::<Vec<_>>()
    };

    let p1 = mixer.request_pad_simple("sink_1").unwrap();
    assert!(
        mixer.request_pad_simple("sink_3").is_none(),
        "sink_3 is past the end"
    );
    let p0 = mixer.request_pad_simple("sink_%u").unwrap();
    let p2 = mixer.request_pad_simple("sink_%u").unwrap();
    assert_eq!(
        names(&[p0.clone(), p1.clone(), p2.clone()]),
        ["sink_0", "sink_1", "sink_2"]
    );
    assert!(mixer.request_pad_simple("sink_%u").is_none(), "full");
    assert!(mixer.static_pad("mix_2").is_some());

    mixer.release_request_pad(&p1);
    assert!(mixer.static_pad("mix_1").is_none());
    let again = mixer.request_pad_simple("sink_%u").unwrap();
    assert_eq!(again.name(), "sink_1");
    assert!(mixer.static_pad("mix_1").is_some());
}
