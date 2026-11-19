// GStreamer multi audio mixer element
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
// Internal AudioAggregator-based mixer element. Its output buffer holds every
// participant's stereo mix side by side: participant N's left and right are
// channels 2N and 2N + 1, for N from 0 to max-participants - 1 (a fixed width,
// so nothing renegotiates when participants come and go). The internal splitter
// then sends each pair to its own output.
//
// Each input adds into every output that hears it, with a left and a right
// gain per (speaker, listener) path taken from the scene (scene.rs). Gains are
// recomputed at most once per output buffer, when something changed, and a
// changed path ramps linearly across that buffer to its new value.
// ---------------------------------------------------------------------------

use gst::glib;
use gst::prelude::*;
use gst::subclass::prelude::*;
use gst_audio::AudioFormat;
use gst_audio::prelude::*;
use gst_audio::subclass::prelude::*;

use std::sync::{Arc, LazyLock, Mutex};

use byte_slice_cast::*;

use crate::dsp::Paths;
use crate::scene::Scene;

#[derive(Default)]
struct State {
    // Caps of the first sinkpad. This defines the allowed caps
    // of all sink pads as well as the srcpad caps
    configured_info: Option<gst_audio::AudioInfo>,

    scene: Option<Arc<Mutex<Scene>>>,

    // Rows (speakers) and width (output channels), from the scene.
    rows: usize,
    width: usize,

    // The paths and their state, once the sample rate is known.
    paths: Option<Paths>,
}

#[derive(Default)]
pub struct MultiMixerElement {
    state: Arc<Mutex<State>>,
}

static CAT: LazyLock<gst::DebugCategory> = LazyLock::new(|| {
    gst::DebugCategory::new(
        "acousticmultimixer-mixer",
        gst::DebugColorFlags::empty(),
        Some("Audio multi mixer element"),
    )
});

#[glib::object_subclass]
impl ObjectSubclass for MultiMixerElement {
    const NAME: &'static str = "GstAcousticMultiMixerMixer";
    type Type = super::MultiMixerElement;
    type ParentType = gst_audio::AudioAggregator;
}

impl ObjectImpl for MultiMixerElement {
    fn constructed(&self) {
        self.parent_constructed();

        self.obj().set_ignore_inactive_pads(true);
    }
}

impl GstObjectImpl for MultiMixerElement {}

impl ElementImpl for MultiMixerElement {
    fn metadata() -> Option<&'static gst::subclass::ElementMetadata> {
        static ELEMENT_METADATA: LazyLock<gst::subclass::ElementMetadata> = LazyLock::new(|| {
            gst::subclass::ElementMetadata::new(
                "Audio multi mixer element",
                "Generic/Audio",
                "Audio multi mixer element",
                "Tim-Philipp Müller <tim@centricular.com>",
            )
        });

        Some(&*ELEMENT_METADATA)
    }

    fn pad_templates() -> &'static [gst::PadTemplate] {
        // TODO: allow other different channel counts, and add more formats
        static PAD_TEMPLATES: LazyLock<Vec<gst::PadTemplate>> = LazyLock::new(|| {
            let input_caps = gst_audio::AudioCapsBuilder::new_interleaved()
                .format_list([gst_audio::AUDIO_FORMAT_S16, gst_audio::AUDIO_FORMAT_F32])
                .rate_range(1..)
                .channels(1) // only support mono for now
                .build();

            let sink_pad_template = gst::PadTemplate::with_gtype(
                "sink_%u",
                gst::PadDirection::Sink,
                gst::PadPresence::Request,
                &input_caps,
                gst_audio::AudioAggregatorPad::static_type(),
            )
            .unwrap();

            // We always use f32 as the internal mixing format for now regardless of the input format
            let output_caps = gst_audio::AudioCapsBuilder::new_interleaved()
                .format(gst_audio::AUDIO_FORMAT_F32)
                .rate_range(1..)
                .channels_range(1..i16::MAX as i32)
                .build();

            let src_pad_template = gst::PadTemplate::with_gtype(
                "src",
                gst::PadDirection::Src,
                gst::PadPresence::Always,
                &output_caps,
                gst_audio::AudioAggregatorPad::static_type(),
            )
            .unwrap();

            vec![sink_pad_template, src_pad_template]
        });

        PAD_TEMPLATES.as_ref()
    }
}

impl AggregatorImpl for MultiMixerElement {
    fn start(&self) -> Result<(), gst::ErrorMessage> {
        let mut state = self.state.lock().unwrap();
        state.configured_info = None;
        drop(state);

        self.parent_start()
    }

    fn stop(&self) -> Result<(), gst::ErrorMessage> {
        let mut state = self.state.lock().unwrap();
        state.configured_info = None;
        state.paths = None;
        drop(state);

        self.parent_stop()
    }

    fn sink_event(&self, aggregator_pad: &gst_base::AggregatorPad, event: gst::Event) -> bool {
        match event.view() {
            gst::EventView::Caps(ev) => {
                let caps = ev.caps_owned();

                let Ok(info) = gst_audio::AudioInfo::from_caps(&caps) else {
                    return false;
                };

                gst::info!(CAT, obj = aggregator_pad, "Have caps {caps}");

                let mut state = self.state.lock().unwrap();
                if let Some(ref configured_info) = state.configured_info {
                    gst::info!(
                        CAT,
                        obj = aggregator_pad,
                        "Configured info {configured_info:?}"
                    );
                    // All sink pads must have the same rate and channel count (but format can differ)
                    if info.rate() != configured_info.rate()
                        || info.channels() != configured_info.channels()
                    {
                        return false;
                    }
                } else {
                    state.configured_info = Some(info);
                }

                self.obj().set_sink_caps(
                    aggregator_pad
                        .downcast_ref::<gst_audio::AudioAggregatorPad>()
                        .unwrap(),
                    &caps,
                );

                true
            }
            _ => self.parent_sink_event(aggregator_pad, event),
        }
    }

    fn sink_query(
        &self,
        aggregator_pad: &gst_base::AggregatorPad,
        query: &mut gst::QueryRef,
    ) -> bool {
        match query.view_mut() {
            gst::QueryViewMut::Caps(q) => {
                let filter = q.filter();

                let state = self.state.lock().unwrap();
                let mut allowed_caps = if let Some(ref configured_info) = state.configured_info {
                    // TODO: would be nice to support stereo, but for now same restriction as below
                    assert_eq!(configured_info.channels(), 1);
                    let caps = configured_info.to_caps().unwrap();
                    drop(state);
                    caps
                } else {
                    drop(state);

                    let caps = self.obj().src_pad().peer_query_caps(None);

                    let caps = caps.intersect_with_mode(
                        &self.obj().src_pad().pad_template_caps(),
                        gst::CapsIntersectMode::First,
                    );

                    if caps.is_empty() {
                        q.set_result(&caps);
                        return true;
                    }

                    caps
                };

                let allowed_caps = allowed_caps.make_mut();
                for s in allowed_caps.iter_mut() {
                    s.set("channels", 1i32); // TODO: would be nice to support stereo
                    s.remove_field("channel-mask");
                    // we accept any input format, not just our mixing format
                    s.remove_field("format");
                }

                let allowed_caps = allowed_caps.intersect_with_mode(
                    &aggregator_pad.pad_template_caps(),
                    gst::CapsIntersectMode::First,
                );

                if let Some(filter) = filter {
                    q.set_result(
                        &filter.intersect_with_mode(&allowed_caps, gst::CapsIntersectMode::First),
                    );
                } else {
                    q.set_result(&allowed_caps);
                }

                true
            }
            gst::QueryViewMut::AcceptCaps(q) => {
                let allowed_caps = aggregator_pad.query_caps(None);
                let caps = q.caps();
                q.set_result(allowed_caps.can_intersect(caps));
                true
            }
            _ => self.parent_sink_query(aggregator_pad, query),
        }
    }

    fn src_query(&self, query: &mut gst::QueryRef) -> bool {
        match query.view_mut() {
            gst::QueryViewMut::Caps(q) => {
                let templ = self.obj().src_pad().pad_template_caps();
                let filter = q.filter();

                if let Some(filter) = filter {
                    q.set_result(
                        &filter.intersect_with_mode(&templ, gst::CapsIntersectMode::First),
                    );
                } else {
                    q.set_result(&templ);
                }

                true
            }
            _ => self.parent_src_query(query),
        }
    }

    fn negotiate(&self) -> bool {
        let state = self.state.lock().unwrap();

        let Some(width) = state.scene.as_ref().map(|s| s.lock().unwrap().width()) else {
            gst::error!(CAT, imp = self, "No scene");
            return false;
        };

        // We require the same caps on every sinkpad so just take the caps of the first
        // sinkpad and update those for the srcpad
        let Some(mut output_caps) = state
            .configured_info
            .as_ref()
            .and_then(|info| info.to_caps().ok())
        else {
            // FIXME: Should probably generate default caps here for force-live?
            gst::warning!(CAT, imp = self, "Have no caps yet");
            return false;
        };

        let mut_ref = output_caps.get_mut().unwrap();
        mut_ref.set("channels", width as i32);
        mut_ref.set("channel-mask", gst::Bitmask::new(0));
        // Our internal mixing format is always f32
        mut_ref.set("format", gst_audio::AUDIO_FORMAT_F32.to_str());

        drop(state);

        gst::debug!(CAT, imp = self, "Setting caps {output_caps:?}");

        // The output width is fixed (the bin's max-participants), so the caps never change once set:
        // when this runs again (an output was linked), negotiated_src_caps sees equal caps and does
        // nothing, so a pending output buffer is never disturbed. None of the upstream minus1mixer's
        // AudioAggregator workarounds are needed.
        if self.parent_negotiated_src_caps(&output_caps).is_ok() {
            self.obj().set_src_caps(&output_caps);
            true
        } else {
            false
        }
    }
}

impl AudioAggregatorImpl for MultiMixerElement {
    // Called before any of the mixing into a new output buffer.
    fn create_output_buffer(&self, num_frames: u32) -> Option<gst::Buffer> {
        let buf = self.parent_create_output_buffer(num_frames)?;

        let mut state = self.state.lock().unwrap();
        let state = &mut *state;
        let (Some(scene), Some(rate)) = (
            state.scene.clone(),
            state.configured_info.as_ref().map(|i| i.rate() as f64),
        ) else {
            return Some(buf);
        };
        if state.paths.as_ref().is_none_or(|p| p.rate() != rate) {
            state.paths = Some(Paths::new(state.rows, state.width, rate));
            scene.lock().unwrap().dirty = true;
        }
        state
            .paths
            .as_mut()
            .unwrap()
            .next_buffer(num_frames as usize, |table| {
                let mut scene = scene.lock().unwrap();
                scene.dirty.then(|| scene.compute(table))
            });

        Some(buf)
    }

    fn aggregate_one_buffer(
        &self,
        pad: &gst_audio::AudioAggregatorPad,
        inbuf: &gst::BufferRef,
        in_offset: u32,
        outbuf: &mut gst::BufferRef,
        out_offset: u32,
        num_frames: u32,
    ) -> bool {
        let mut state = self.state.lock().unwrap();
        let width = state.width;
        let rows = state.rows;
        let Some(paths) = state.paths.as_mut() else {
            return true;
        };

        let Some(n) = pad
            .name()
            .strip_prefix("sink_")
            .and_then(|n| n.parse::<usize>().ok())
            .filter(|&n| n < rows)
        else {
            return true;
        };

        let audio_info = pad.audio_info().expect("audioinfo");
        assert_eq!(audio_info.channels(), 1);

        let in_offset = in_offset as usize;
        let out_offset = out_offset as usize;
        let num_frames = num_frames as usize;

        gst::log!(
            CAT,
            imp = self,
            "input {n}: frames {num_frames} from {in_offset} to {out_offset}"
        );

        let mut out_map = outbuf.map_writable().unwrap();
        let out = out_map.as_mut_slice_of::<f32>().unwrap();
        let out = &mut out[out_offset * width..][..num_frames * width];
        let in_map = inbuf.map_readable().unwrap();

        match audio_info.format() {
            AudioFormat::F32le => {
                let input = &in_map.as_slice_of::<f32>().unwrap()[in_offset..][..num_frames];
                paths.mix(n, input, 1.0, out, out_offset);
            }
            AudioFormat::S16le => {
                let input = &in_map.as_slice_of::<i16>().unwrap()[in_offset..][..num_frames];
                paths.mix(n, input, 1.0 / 32768.0, out, out_offset);
            }
            format => unimplemented!("{format:?}"),
        }

        true
    }
}

impl MultiMixerElement {
    /// Gives the mixer its scene, which fixes its output width. Called once, by the bin, before it runs.
    pub(crate) fn set_scene(&self, scene: Arc<Mutex<Scene>>) {
        let mut state = self.state.lock().unwrap();
        {
            let s = scene.lock().unwrap();
            state.rows = s.participants.len();
            state.width = s.width();
        }
        state.paths = None;
        state.scene = Some(scene);
    }
}
