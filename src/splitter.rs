// GStreamer multi audio mixer splitter
//
// Copyright (C) 2022-2026 Tim-Philipp Müller <tim centricular com>
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

// ---------------------------------------------------------------------------
// Internal companion element to the internal AudioAggregator-based mixer
// element that creates output buffers containing the N:M mix results for
// all M outputs. This splitter element then splits those buffers into M
// output buffers and pushes them out on the right pads.
//
// The acousticmultimixer element is then a bin containing mixer ! splitter
// ---------------------------------------------------------------------------

use gst::glib;
use gst::prelude::*;
use gst::subclass::prelude::*;
use gst_audio::AudioFormat;

use byte_slice_cast::*;

use std::collections::BTreeMap;
use std::sync::{Arc, LazyLock, Mutex};

const DEFAULT_SAMPLE_RATE: i32 = 48000;

// Every output is stereo: output N is channels 2N and 2N + 1 of the input.
const OUTPUT_CHANNELS: usize = 2;

#[derive(Debug, Clone, Copy)]
struct Settings {
    pub sample_rate: i32,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            sample_rate: DEFAULT_SAMPLE_RATE,
        }
    }
}

#[derive(Debug)]
struct OutputStream {
    pad: gst::Pad,

    // Negotiated output format
    format: Option<gst_audio::AudioFormat>,
}

impl OutputStream {
    // stream-start, segment, caps events have been sent
    fn inited(&self) -> bool {
        self.format.is_some()
    }

    fn set_up(&mut self, sink_pad: &gst::Pad, audio_info: &gst_audio::AudioInfo, n_channels: i32) {
        gst::info!(CAT, obj = self.pad, "Setting up output pad..");

        // Query caps to determine desired output format
        let caps = self.pad.peer_query_caps(None);

        let mut caps =
            caps.intersect_with_mode(&self.pad.pad_template_caps(), gst::CapsIntersectMode::First);

        gst::log!(CAT, obj = self.pad, "Caps {caps}");

        caps.truncate();

        let format = if let Some(s) = caps.structure(0) {
            if let Ok(format_str) = s.get::<&str>("format") {
                gst_audio::AudioFormat::from_string(format_str)
            } else if let Ok(formats) = s.get::<gst::List>("format")
                && let Some(first) = formats.first()
            {
                gst_audio::AudioFormat::from_string(first.get::<&str>().expect("format string"))
            } else {
                audio_info.format()
            }
        } else {
            audio_info.format()
        };

        self.format = Some(format);

        gst::info!(CAT, obj = self.pad, "Negotiated output format {format}");

        // Forward stream start event
        if let Some(stream_start) = sink_pad.sticky_event::<gst::event::StreamStart>(0) {
            self.pad.push_event(stream_start);
        }

        // Send caps event (Note: audio_info contains sink pad number of channels)
        let caps = gst_audio::AudioCapsBuilder::new_interleaved()
            .format(format)
            .channels(n_channels)
            .rate(audio_info.rate() as i32)
            .build();
        self.pad.push_event(gst::event::Caps::new(&caps));

        // Forward segment event
        if let Some(segment) = sink_pad.sticky_event::<gst::event::Segment>(0) {
            self.pad.push_event(segment);
        }
    }
}

#[derive(Debug, Default)]
struct State {
    // Input format
    audio_info: Option<gst_audio::AudioInfo>,

    // Next pad number to try when a new src pad is requested
    pad_serial: u32,

    // Pad numbers and pads in use
    // IMPROVE: use vec-collections based on SmallVec perhaps?
    output_streams: BTreeMap<u32, OutputStream>,

    flow_combiner: gst_base::UniqueFlowCombiner,
}

#[derive(Default)]
pub struct Splitter {
    state: Arc<Mutex<State>>,
    settings: Mutex<Settings>,
}

static CAT: LazyLock<gst::DebugCategory> = LazyLock::new(|| {
    gst::DebugCategory::new(
        "acousticmultimixer-splitter",
        gst::DebugColorFlags::empty(),
        Some("Audio multi mixer splitter"),
    )
});

#[glib::object_subclass]
impl ObjectSubclass for Splitter {
    const NAME: &'static str = "GstAcousticMultiMixerSplitter";
    type Type = super::MultiMixerSplitter;
    type ParentType = gst::Element;
}

impl ObjectImpl for Splitter {
    fn constructed(&self) {
        self.parent_constructed();

        let sink_templ = self.obj().pad_template("sink").unwrap();

        let sink_pad = gst::PadBuilder::<gst::Pad>::from_template(&sink_templ)
            .name("sink")
            .chain_function(|pad, parent, buffer| {
                Splitter::catch_panic_pad_function(
                    parent,
                    || Err(gst::FlowError::Error),
                    |splitter| splitter.sink_chain(pad, buffer),
                )
            })
            .event_function(|pad, parent, event| {
                Splitter::catch_panic_pad_function(
                    parent,
                    || false,
                    |splitter| splitter.sink_event(pad, event),
                )
            })
            .query_function(|pad, parent, query| {
                Splitter::catch_panic_pad_function(
                    parent,
                    || false,
                    |splitter| splitter.sink_query(pad, query),
                )
            })
            .build();

        self.obj().add_pad(&sink_pad).unwrap();
    }

    fn properties() -> &'static [glib::ParamSpec] {
        static PROPERTIES: LazyLock<Vec<glib::ParamSpec>> = LazyLock::new(|| {
            vec![
                glib::ParamSpecInt::builder("sample-rate")
                    .nick("Sample Rate")
                    .blurb("Sample rate to use")
                    .minimum(1i32)
                    .maximum(i32::MAX)
                    .default_value(DEFAULT_SAMPLE_RATE)
                    .mutable_ready()
                    .build(),
            ]
        });

        PROPERTIES.as_ref()
    }

    fn set_property(&self, _id: usize, value: &glib::Value, pspec: &glib::ParamSpec) {
        match pspec.name() {
            "sample-rate" => {
                let mut settings = self.settings.lock().unwrap();
                settings.sample_rate = value.get::<i32>().unwrap();
            }
            name => unimplemented!("{name}"),
        }
    }

    fn property(&self, _id: usize, pspec: &glib::ParamSpec) -> glib::Value {
        match pspec.name() {
            "sample-rate" => {
                let settings = self.settings.lock().unwrap();
                settings.sample_rate.to_value()
            }
            name => unimplemented!("{name}"),
        }
    }
}

impl GstObjectImpl for Splitter {}

impl ElementImpl for Splitter {
    fn metadata() -> Option<&'static gst::subclass::ElementMetadata> {
        static ELEMENT_METADATA: LazyLock<gst::subclass::ElementMetadata> = LazyLock::new(|| {
            gst::subclass::ElementMetadata::new(
                "Audio multi mixer splitter",
                "Generic/Audio",
                "Audio multi mixer splitter",
                "Tim-Philipp Müller <tim@centricular.com>",
            )
        });

        Some(&*ELEMENT_METADATA)
    }

    fn pad_templates() -> &'static [gst::PadTemplate] {
        static PAD_TEMPLATES: LazyLock<Vec<gst::PadTemplate>> = LazyLock::new(|| {
            let sink_pad_template = gst::PadTemplate::new(
                "sink",
                gst::PadDirection::Sink,
                gst::PadPresence::Always,
                &gst_audio::AudioCapsBuilder::new_interleaved()
                    .format(gst_audio::AUDIO_FORMAT_F32)
                    .rate_range(1..)
                    .channels_range(1..i16::MAX as i32)
                    .build(),
            )
            .unwrap();

            let src_pad_template = gst::PadTemplate::new(
                "src_%u",
                gst::PadDirection::Src,
                gst::PadPresence::Request,
                &gst_audio::AudioCapsBuilder::new_interleaved()
                    .format_list([gst_audio::AUDIO_FORMAT_S16, gst_audio::AUDIO_FORMAT_F32])
                    .rate_range(1..)
                    .channels(OUTPUT_CHANNELS as i32)
                    .build(),
            )
            .unwrap();

            vec![sink_pad_template, src_pad_template]
        });

        PAD_TEMPLATES.as_ref()
    }

    fn request_new_pad(
        &self,
        templ: &gst::PadTemplate,
        name: Option<&str>,
        _caps: Option<&gst::Caps>,
    ) -> Option<gst::Pad> {
        assert!(name.is_none() || name.unwrap().starts_with("src_"));

        let mut state = self.state.lock().unwrap();

        let pad_name = name.unwrap_or("src_%u");

        let name_suffix = pad_name.strip_prefix("src_").unwrap();

        let n = if name_suffix == "%u" {
            // Find next number that's not is already taken
            loop {
                let n = state.pad_serial;
                state.pad_serial += 1;
                if !state.output_streams.contains_key(&n) {
                    break n;
                }
            }
        } else {
            let n = name_suffix.parse::<u32>().unwrap();

            // Error out if requested number is already in use
            if state.output_streams.contains_key(&n) {
                gst::error!(CAT, imp = self, "Pad src_{n} exists already!");
                return None;
            }

            n
        };

        let new_name = format!("src_{n}");
        let new_pad = gst::Pad::builder_from_template(templ)
            .name(&new_name)
            .build();
        self.obj().add_pad(&new_pad).unwrap();
        new_pad.set_active(true).unwrap();

        state.output_streams.insert(
            n,
            OutputStream {
                pad: new_pad.clone(),
                format: None,
            },
        );

        state.flow_combiner.add_pad(&new_pad);

        gst::info!(CAT, imp = self, "New output pad {new_name}",);

        Some(new_pad)
    }

    fn release_pad(&self, pad: &gst::Pad) {
        gst::info!(CAT, imp = self, "Releasing output pad {}", pad.name());

        let mut state = self.state.lock().unwrap();

        state
            .output_streams
            .retain(|_, output_stream| output_stream.pad != *pad);

        state.flow_combiner.remove_pad(pad);

        pad.set_active(false).unwrap();
        self.obj().remove_pad(pad).unwrap();
    }
}

trait ConvertFromF32: Sized + Copy + FromByteSlice {
    fn from_f32(val: f32) -> Self
    where
        Self: Sized;
}

impl ConvertFromF32 for f32 {
    fn from_f32(val: f32) -> f32 {
        val
    }
}

impl ConvertFromF32 for i16 {
    fn from_f32(val: f32) -> i16 {
        // This will saturate/clamp and convert NaN to 0 automatically, can't panic
        val as i16
    }
}

impl Splitter {
    fn sink_chain(
        &self,
        sink_pad: &gst::Pad,
        in_buf: gst::Buffer,
    ) -> Result<gst::FlowSuccess, gst::FlowError> {
        let mut combined_flow = Ok(gst::FlowSuccess::Ok);

        gst::log!(CAT, obj = sink_pad, "Handling buffer {:?}", in_buf);

        let mut state = self.state.lock().unwrap();

        let State {
            audio_info,
            pad_serial: _,
            output_streams,
            flow_combiner,
        } = &mut *state;

        let Some(audio_info) = audio_info.as_ref() else {
            return Err(gst::FlowError::NotNegotiated);
        };

        let in_map = in_buf.map_readable().unwrap();
        let in_samples = in_map.as_slice_of::<f32>().unwrap();

        let n_channels_in = audio_info.channels() as usize;
        let bps = std::mem::size_of::<f32>();
        let n_samples = in_map.size() / (bps * n_channels_in);
        assert_eq!(in_map.size(), n_samples * bps * n_channels_in);

        gst::trace!(
            CAT,
            imp = self,
            "Got buffer with {n_samples} samples, {n_channels_in} input channels",
        );

        // The input buffer holds every output's mix side by side (fixed layout): output N is
        // OUTPUT_CHANNELS channels from channel N × OUTPUT_CHANNELS.
        for (&output_num, output_stream) in output_streams.iter_mut() {
            let channel_offset = output_num as usize * OUTPUT_CHANNELS;
            if channel_offset + OUTPUT_CHANNELS > n_channels_in {
                gst::warning!(
                    CAT,
                    imp = self,
                    "No channels for src_{output_num} in a {n_channels_in}-channel input"
                );
                continue;
            }

            if !output_stream.inited() {
                output_stream.set_up(sink_pad, audio_info, OUTPUT_CHANNELS as i32);
            }

            fn split_output_buf<T: ConvertFromF32>(
                n_channels_in: usize,
                channel_offset: usize,
                n_samples: usize,
                in_samples: &[f32],
                conv_scale: f32,
            ) -> gst::Buffer {
                let bps = std::mem::size_of::<T>();

                let in_frames = in_samples.chunks_exact(n_channels_in);

                let mut out_buf =
                    gst::Buffer::with_size(n_samples * OUTPUT_CHANNELS * bps).unwrap();
                let mut_buf = out_buf.get_mut().unwrap();
                let mut out_map = mut_buf.map_writable().unwrap();
                let out_samples = out_map.as_mut_slice_of::<T>().unwrap();
                let (out_frames, _) = out_samples.as_chunks_mut::<OUTPUT_CHANNELS>();

                assert_eq!(out_frames.len(), in_frames.len());

                for (in_frame, out_frame) in Iterator::zip(in_frames, out_frames.iter_mut()) {
                    let in_frame_out_samples = &in_frame[channel_offset..][..OUTPUT_CHANNELS];

                    for (in_sample, out_sample) in
                        Iterator::zip(in_frame_out_samples.iter(), out_frame.iter_mut())
                    {
                        *out_sample = T::from_f32(*in_sample * conv_scale);
                    }
                }

                drop(out_map);

                out_buf
            }

            let output_buf = {
                let mut out_buf = match output_stream.format.expect("output format") {
                    AudioFormat::F32le | AudioFormat::F32be => split_output_buf::<f32>(
                        n_channels_in,
                        channel_offset,
                        n_samples,
                        in_samples,
                        1.0,
                    ),

                    AudioFormat::S16le | AudioFormat::S16be => split_output_buf::<i16>(
                        n_channels_in,
                        channel_offset,
                        n_samples,
                        in_samples,
                        32768.0,
                    ),

                    format => unimplemented!("{format:?}"),
                };

                let _ = in_buf.copy_into(
                    out_buf.get_mut().unwrap(),
                    gst::BufferCopyFlags::FLAGS
                        | gst::BufferCopyFlags::META
                        | gst::BufferCopyFlags::TIMESTAMPS,
                    ..,
                );

                out_buf
            };

            let out_pad = &output_stream.pad;

            let flow = out_pad.push(output_buf);

            combined_flow = flow_combiner.update_pad_flow(&out_pad.clone(), flow);

            gst::trace!(
                CAT,
                obj = out_pad,
                "flow: {flow:?}, combined flow now: {combined_flow:?}"
            );
        }

        // Allow no pads and all output pads unlinked
        if combined_flow == Err(gst::FlowError::NotLinked) {
            combined_flow = Ok(gst::FlowSuccess::Ok);
        }

        combined_flow
    }

    fn sink_event(&self, pad: &gst::Pad, event: gst::Event) -> bool {
        use gst::EventView;

        gst::trace!(CAT, obj = pad, "Handling event {:?}", event);

        match event.view() {
            EventView::Caps(c) => {
                let caps = c.caps_owned();

                gst::info!(CAT, obj = pad, "input caps: {caps}");

                let mut state = self.state.lock().unwrap();
                state.audio_info = match gst_audio::AudioInfo::from_caps(&caps) {
                    Ok(info) => {
                        gst::info!(CAT, obj = pad, "{info:?}");
                        Some(info)
                    }
                    Err(_) => {
                        gst::error!(CAT, obj = pad, "Failed to parse input caps {caps}");
                        return false;
                    }
                };
            }

            // Drop input segment event here, since we don't want to forward it to any output pads
            // before caps have been set on those pads. We will forward the segment later when we
            // set up each output pad.
            EventView::Segment(s) => {
                gst::info!(CAT, obj = pad, "input segment: {s:?}");
            }

            EventView::FlushStop(_) => {
                let mut state = self.state.lock().unwrap();
                state.flow_combiner.reset();
            }

            _ => {
                // Ignore return value here, buffer pushes will sort it out via flow returns
                let _ = gst::Pad::event_default(pad, Some(&*self.obj()), event);
            }
        }

        true
    }

    #[allow(clippy::single_match)]
    fn sink_query(&self, pad: &gst::Pad, query: &mut gst::QueryRef) -> bool {
        if !gst::Pad::query_default(pad, Some(&*self.obj()), query) {
            return false;
        }

        match query.view_mut() {
            // Caps query: only allow our configured sample rate (we do not want to negotiate this)
            gst::QueryViewMut::Caps(query) => {
                if let Some(mut caps) = query.result_owned() {
                    let caps_ref = caps.make_mut();

                    let rate = {
                        let settings = self.settings.lock().unwrap();
                        settings.sample_rate
                    };

                    caps_ref.set("rate", rate);
                    query.set_result(&Some(caps));
                }

                return true;
            }
            _ => (),
        }

        true
    }
}
