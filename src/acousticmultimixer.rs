// GStreamer acoustic multi mixer
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

/**
 * SECTION:element-acousticmultimixer
 *
 * Mix-minus mixer with positions: every participant's voice comes in on a "sink_%u" pad, and
 * what they hear comes out of the matching "mix_%u" pad: everyone else, in stereo, placed by
 * where they are relative to the listener.
 *
 * Each request for "sink_%u" also creates "mix_%u"; releasing "sink_%u" removes both. Pads
 * can be added and removed at any time. Their number is limited by the construct-only
 * "max-participants" property (default 32): pads are numbered 0 to max-participants - 1, and
 * the internal mix is always that wide, so adding or removing a pad never renegotiates. A
 * participant who only listens never feeds their sink pad; one who only talks leaves their
 * output unlinked, and it is not mixed.
 *
 * Positions and settings are properties of the sink pads: "x", "y", "z" (x east, y north,
 * z up) and "yaw", "pitch", "roll" in degrees (0 faces +x; yaw turns left, pitch looks up,
 * roll lowers the right ear); "listener-x" … "listener-roll" to hear from somewhere else
 * than the voice; "gain" and "mute" for everyone hearing the voice; "trims" for this
 * listener's own gains for others (e.g. trims,sink_0=0.0,sink_2=0.5). A participant with no
 * position (none of x … roll ever set) is heard centred at full volume, and hears everyone
 * that way. Level falls with distance by the element's "distance-model",
 * "reference-distance", "rolloff" and "max-range" (fading out smoothly from "taper" × max-range,
 * by default the last quarter, so voices do not cut off), and distant voices lose high frequencies
 * ("air-absorption"). Direction ("direction-model"): "head" (default) gives the far ear the
 * voice later (up to 0.66 ms), quieter and duller, as a head between the ears would, and
 * voices behind duller in both ears ("rear-shadow"), as the outer ears make them; "pan" is a
 * constant-power stereo pan (silent in the far ear at the side; with air-absorption=0, the same
 * as SpatialAudioServer's matrix engine). "room" adds the early reflections of a kind of space
 * ("outdoor", the default, has none): a few echoes after each voice, the later ones heard more
 * from all around, then a decaying tail (late reverb, its level adjusted by "reverb-level"),
 * relatively louder the further away the voice is, and quieter closer than "near-distance".
 * Changes are taken once per output buffer and ramp across it, so moving does not click.
 *
 * The sample rate is not negotiated, as this is not possible to do in a race-free manner. It
 * must instead be configured at element creation time via the "sample-rate" construct-only
 * property. The default sample rate is 48kHz.
 *
 * Inputs are mono, outputs stereo, each F32 or S16 (they may differ); mixing is in F32.
 *
 * All outputs are pushed from one streaming thread, so when more than one output is linked,
 * put a queue after each (as after a tee); otherwise the first sink to preroll blocks the rest.
 *
 * Built on gst-plugins-rs's minus1mixer; reflections and reverb come next (PLAN.md).
 *
 * ## Example pipelines
 *
 * |[
 * gst-launch-1.0 acousticmultimixer name=m sink_1::y=4 sink_2::x=8 \
 *   audiotestsrc freq=440 ! m.sink_0 \
 *   audiotestsrc wave=ticks ! m.sink_1 \
 *   audiotestsrc freq=220 ! m.sink_2  \
 *   m.mix_0 ! audioconvert ! autoaudiosink
 * ]| What participant 0 hears, at the origin facing +x: the ticks 4 m to the left, the 220 Hz
 * tone 8 m ahead and quieter.
 */
use gst::glib;
use gst::prelude::*;
use gst::subclass::prelude::*;

use std::sync::{Arc, LazyLock, Mutex};

use crate::dimensions::{
    ABSORPTION_PROPERTIES, Contents, DERIVED_PROPERTIES, MATERIAL_PROPERTIES, Material,
    SIZE_PROPERTIES,
};
use crate::rooms::MAX_TAP_DELAY_MS;
use crate::scene::Scene;
use crate::spatial::{Acoustics, DirectionModel, DistanceModel, DistanceParams};

/// The properties that override a value of the room's preset (−1, or an empty list: the preset's).
const ROOM_OVERRIDES: [&str; 10] = [
    "reflection-delays",
    "reflection-levels",
    "wall-cutoff",
    "reverb-decay",
    "reverb-treble-decay",
    "reverb-size",
    "room-falloff",
    "reflection-spread",
    "reflection-diffusion",
    "near-floor",
];

/// A property overriding one of the room preset's numbers: −1 (the default) leaves the preset's.
fn room_override(name: &str, nick: &str, blurb: &str, maximum: f64) -> glib::ParamSpec {
    glib::ParamSpecDouble::builder(name)
        .nick(nick)
        .blurb(blurb)
        .minimum(-1.0)
        .maximum(maximum)
        .default_value(-1.0)
        .mutable_playing()
        .build()
}

/// A property overriding the preset's list of tap delays or levels: empty (the default) leaves the preset's.
fn room_override_list(
    name: &str,
    nick: &str,
    blurb: &str,
    minimum: f64,
    maximum: f64,
) -> glib::ParamSpec {
    gst::ParamSpecArray::builder(name)
        .nick(nick)
        .blurb(blurb)
        .element_spec(
            &glib::ParamSpecDouble::builder("value")
                .minimum(minimum)
                .maximum(maximum)
                .build(),
        )
        .mutable_playing()
        .build()
}

/// One of the room's sizes. With all three given, the room's values come from its dimensions (where no property
/// overrides them) instead of its preset.
fn room_size(name: &str, nick: &str, blurb: &str) -> glib::ParamSpec {
    glib::ParamSpecDouble::builder(name)
        .nick(nick)
        .blurb(&format!(
            "{blurb}; with width, depth and height given, the room's reflections and reverb come from its size, \
             materials and contents instead of its preset"
        ))
        .minimum(0.0)
        .maximum(1000.0)
        .default_value(0.0)
        .mutable_playing()
        .build()
}

fn material(name: &str, nick: &str, blurb: &str, default: Material) -> glib::ParamSpec {
    glib::ParamSpecEnum::builder_with_default(name, default)
        .nick(nick)
        .blurb(&format!("{blurb}, with the room's dimensions"))
        .mutable_playing()
        .build()
}

fn absorption(name: &str, nick: &str, what: &str) -> glib::ParamSpec {
    gst::ParamSpecArray::builder(name)
        .nick(nick)
        .blurb(&format!(
            "How much sound {what} absorb (0-1), overriding their material: <bass, treble>, or one value for both; \
             empty = the material's"
        ))
        .element_spec(
            &glib::ParamSpecDouble::builder("value")
                .minimum(0.0)
                .maximum(1.0)
                .build(),
        )
        .mutable_playing()
        .build()
}

/// The numbers in a list property (accepting integers too, as "<10, 20>" parses to them).
fn numbers(value: &glib::Value) -> Vec<f64> {
    let Ok(array) = value.get::<gst::Array>() else {
        return vec![];
    };
    array
        .iter()
        .filter_map(|v| {
            v.get::<f64>()
                .ok()
                .or_else(|| v.get::<i32>().ok().map(f64::from))
                .or_else(|| v.get::<f32>().ok().map(f64::from))
        })
        .collect()
}

/// −1 (or any negative value) is "the preset's".
fn optional(value: &glib::Value) -> Option<f64> {
    let v = value.get::<f64>().unwrap();
    (v >= 0.0).then_some(v)
}

static CAT: LazyLock<gst::DebugCategory> = LazyLock::new(|| {
    gst::DebugCategory::new(
        "acousticmultimixer",
        gst::DebugColorFlags::empty(),
        Some("Acoustic multi mixer"),
    )
});

const DEFAULT_SAMPLE_RATE: i32 = 48000;
const DEFAULT_MAX_PARTICIPANTS: u32 = 32;
const MAX_MAX_PARTICIPANTS: u32 = 256;

#[derive(Debug, Clone, Copy)]
struct Settings {
    pub sample_rate: i32,

    // Number of input/output pairs, fixed at creation so the mixer's output never changes
    // width (no renegotiation while running). Pads are numbered 0 to max_participants - 1.
    pub max_participants: u32,

    // Need to store force_live which is construct-only so we can set it later
    // on the mixer in the constructed vfunc when we create the mixer.
    pub force_live: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            sample_rate: DEFAULT_SAMPLE_RATE,
            max_participants: DEFAULT_MAX_PARTICIPANTS,
            force_live: false,
        }
    }
}

struct State {
    // internal mixer element
    mixer_element: gst::Element,
    // internal multimixersplitter
    splitter: gst::Element,
    // streams
    streams: Vec<u32>,
    // fixed number of outputs (max-participants)
    max_participants: u32,
    // participants and their settings, shared with the pads and the mixer
    scene: Arc<Mutex<Scene>>,
}

#[derive(Default)]
pub struct AcousticMultiMixer {
    state: Arc<Mutex<Option<State>>>,
    settings: Mutex<Settings>,
}

#[glib::object_subclass]
impl ObjectSubclass for AcousticMultiMixer {
    const NAME: &'static str = "GstAcousticMultiMixer";
    type Type = super::AcousticMultiMixer;
    type ParentType = gst::Bin;
    type Interfaces = (gst::ChildProxy,);
}

// The children are the sink pads (not the internal elements, as for a plain bin), so that
// gst-launch can set pad properties as with compositor: acousticmultimixer sink_0::x=2.
impl ChildProxyImpl for AcousticMultiMixer {
    fn child_by_name(&self, name: &str) -> Option<glib::Object> {
        self.obj()
            .static_pad(name)
            .filter(|pad| pad.direction() == gst::PadDirection::Sink)
            .map(|pad| pad.upcast())
    }

    fn child_by_index(&self, index: u32) -> Option<glib::Object> {
        self.obj()
            .sink_pads()
            .get(index as usize)
            .map(|pad| pad.clone().upcast())
    }

    fn children_count(&self) -> u32 {
        self.obj().num_sink_pads() as u32
    }
}

impl BinImpl for AcousticMultiMixer {}

impl ObjectImpl for AcousticMultiMixer {
    fn constructed(&self) {
        self.parent_constructed();

        let obj = self.obj();

        let settings = self.settings.lock().unwrap();

        let mixer_element = glib::object::Object::builder::<crate::MultiMixerElement>()
            .property("force-live", settings.force_live)
            .build();

        let splitter = glib::object::Object::builder::<crate::MultiMixerSplitter>()
            .property("sample-rate", settings.sample_rate)
            .build();

        obj.add(&mixer_element).unwrap();
        obj.add(&splitter).unwrap();

        let scene = Arc::new(Mutex::new(Scene::new(settings.max_participants as usize)));
        mixer_element.imp().set_scene(scene.clone());

        mixer_element.link(&splitter).unwrap();

        for prop_name in [
            "ignore-inactive-pads",
            "discont-wait",
            "alignment-threshold",
            "output-buffer-duration-fraction",
            "output-buffer-duration",
            "start-time",
            "start-time-selection",
            "min-upstream-latency",
            "latency",
        ] {
            obj.bind_property(prop_name, &mixer_element, prop_name)
                .build();
        }

        let mut state_guard = self.state.lock().unwrap();

        let state = State {
            mixer_element: mixer_element.into(),
            splitter: splitter.into(),
            streams: vec![],
            max_participants: settings.max_participants,
            scene,
        };

        *state_guard = Some(state);
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
                    .construct_only()
                    .build(),

                glib::ParamSpecUInt::builder("max-participants")
                    .nick("Maximum participants")
                    .blurb("Number of sink_N/mix_N pairs available (N from 0 to max-participants - 1); \
                            fixed so that nothing renegotiates while running")
                    .minimum(1)
                    .maximum(MAX_MAX_PARTICIPANTS)
                    .default_value(DEFAULT_MAX_PARTICIPANTS)
                    .construct_only()
                    .build(),

                glib::ParamSpecEnum::builder_with_default("distance-model", DistanceParams::default().model)
                    .nick("Distance model")
                    .blurb("How volume falls with distance beyond reference-distance")
                    .mutable_playing()
                    .build(),

                glib::ParamSpecDouble::builder("reference-distance")
                    .nick("Reference distance")
                    .blurb("Voices this close or closer are heard at full volume")
                    .minimum(0.0)
                    .maximum(1e9)
                    .default_value(DistanceParams::default().reference)
                    .mutable_playing()
                    .build(),

                glib::ParamSpecDouble::builder("rolloff")
                    .nick("Rolloff")
                    .blurb("How fast volume falls beyond reference-distance (see distance-model)")
                    .minimum(0.0)
                    .maximum(1e3)
                    .default_value(DistanceParams::default().rolloff)
                    .mutable_playing()
                    .build(),

                glib::ParamSpecDouble::builder("max-range")
                    .nick("Maximum range")
                    .blurb("Voices further away than this are silent")
                    .minimum(0.0)
                    .maximum(1e9)
                    .default_value(DistanceParams::default().max_range)
                    .mutable_playing()
                    .build(),

                glib::ParamSpecDouble::builder("taper")
                    .nick("Taper")
                    .blurb("Where voices start fading out, as a fraction of max-range; silent at max-range (1 = no fade)")
                    .minimum(0.0)
                    .maximum(1.0)
                    .default_value(DistanceParams::default().taper)
                    .mutable_playing()
                    .build(),

                glib::ParamSpecEnum::builder_with_default("direction-model", Acoustics::default().direction)
                    .nick("Direction model")
                    .blurb("How direction is heard: a stereo pan, or a head (the far ear later, quieter, duller)")
                    .mutable_playing()
                    .build(),

                glib::ParamSpecDouble::builder("air-absorption")
                    .nick("Air absorption")
                    .blurb("High-frequency loss with distance, dB per metre at 8 kHz (air: about 0.1; 0 = none)")
                    .minimum(0.0)
                    .maximum(10.0)
                    .default_value(Acoustics::default().air_absorption)
                    .mutable_playing()
                    .build(),

                glib::ParamSpecEnum::builder_with_default("room", Acoustics::default().room)
                    .nick("Room")
                    .blurb("The space every voice is in, for its early reflections and late reverb: outdoor (none), tiny-room, small-room, big-room, hall, long-hall, cave, cathedral")
                    .mutable_playing()
                    .build(),

                glib::ParamSpecDouble::builder("near-distance")
                    .nick("Near distance")
                    .blurb("Closer than this (m) a voice's reflections and reverb are turned down, 6 dB per halving to 0.5 m, so it stands out from the room (0 = never)")
                    .minimum(0.0)
                    .maximum(1e3)
                    .default_value(Acoustics::default().near_distance)
                    .mutable_playing()
                    .build(),

                glib::ParamSpecDouble::builder("reverb-level")
                    .nick("Reverb level")
                    .blurb("The room's late reverb against its usual level (dB); -60 = none")
                    .minimum(crate::spatial::REVERB_OFF)
                    .maximum(12.0)
                    .default_value(Acoustics::default().reverb_level)
                    .mutable_playing()
                    .build(),

                glib::ParamSpecDouble::builder("rear-shadow")
                    .nick("Rear shadow")
                    .blurb("Head model: low-pass cut-off (Hz) for a source straight behind, easing to none at the side (0 = none)")
                    .minimum(0.0)
                    .maximum(20000.0)
                    .default_value(Acoustics::default().rear_shadow)
                    .mutable_playing()
                    .build(),

                // Overrides of the room preset's values (rooms.rs)

                room_override_list(
                    "reflection-delays",
                    "Reflection delays",
                    "The early reflections' delays after the direct sound (ms), overriding the room preset's; paired \
                     in order with reflection-levels (as many reflections as the shorter list, up to 16), e.g. \
                     <15.0, 25.0, 40.0, 70.0>; empty = the preset's",
                    0.0,
                    MAX_TAP_DELAY_MS,
                ),
                room_override_list(
                    "reflection-levels",
                    "Reflection levels",
                    "The early reflections' levels (dB, against a voice at the reference distance), overriding the \
                     room preset's; empty = the preset's",
                    -80.0,
                    12.0,
                ),
                room_override(
                    "wall-cutoff",
                    "Wall cut-off",
                    "Low-pass cut-off for the reflections (Hz): the walls' treble absorption (0 = none; -1 = the room \
                     preset's)",
                    24000.0,
                ),
                room_override(
                    "reverb-decay",
                    "Reverb decay",
                    "Time for the late reverb to fall by 60 dB in the bass (s; at least 0.05; -1 = the room preset's)",
                    20.0,
                ),
                room_override(
                    "reverb-treble-decay",
                    "Reverb treble decay",
                    "Time for the late reverb to fall by 60 dB in the treble (s; at most reverb-decay; -1 = the room \
                     preset's)",
                    20.0,
                ),
                room_override(
                    "reverb-size",
                    "Reverb size",
                    "The late reverb's mean delay length (ms): about the time between walls, and when the tail \
                     starts (at least 1; -1 = the room preset's)",
                    250.0,
                ),
                room_override(
                    "room-falloff",
                    "Room falloff",
                    "How fast reflections and reverb fall with distance, as a share of the direct sound's fall in dB \
                     (1 = as fast; less: far voices sound roomier; -1 = 0.75)",
                    2.0,
                ),
                room_override(
                    "reflection-spread",
                    "Reflection spread",
                    "How far apart the reflections heard from all around are in the two ears, as a share of each \
                     delay (-1 = 0.08)",
                    0.5,
                ),
                room_override(
                    "reflection-diffusion",
                    "Reflection diffusion",
                    "The first reflection's share heard from all around rather than from the voice's direction, \
                     rising evenly to all of it for the last (-1 = one over the number of reflections)",
                    1.0,
                ),
                room_override(
                    "near-floor",
                    "Near floor",
                    "Where near-distance's turn-down of a close voice's room stops (m; -1 = 0.5)",
                    100.0,
                ),
                // The room's dimensions (dimensions.rs), which take the place of the preset's values

                room_size("room-width", "Room width", "The room's size east to west (m; 0 = not given)"),
                room_size("room-depth", "Room depth", "The room's size north to south (m; 0 = not given)"),
                room_size("room-height", "Room height", "The room's height (m; 0 = not given)"),
                material("wall-material", "Wall material", "What the walls are made of", Material::Plaster),
                material("floor-material", "Floor material", "What the floor is made of", Material::Wood),
                material("ceiling-material", "Ceiling material", "What the ceiling is made of", Material::Plaster),
                absorption("wall-absorption", "Wall absorption", "the walls"),
                absorption("floor-absorption", "Floor absorption", "the floor"),
                absorption("ceiling-absorption", "Ceiling absorption", "the ceiling"),
                glib::ParamSpecEnum::builder_with_default("room-contents", Contents::default())
                    .nick("Room contents")
                    .blurb("What is in the room, absorbing sound (furniture and people), with its dimensions")
                    .mutable_playing()
                    .build(),

                glib::ParamSpecBoxed::builder::<gst::Structure>("room-parameters")
                    .nick("Room parameters")
                    .blurb("The room's values in effect, its preset's with any overrides, and which are overridden \
                            (field overridden: the property names)")
                    .read_only()
                    .build(),

                // AudioAggregator properties we proxy for convenience and discoverability

                glib::ParamSpecBoolean::builder("force-live")
                    .nick("Force live")
                    .blurb("Always operate in live mode and aggregate on timeout regardless of whether any live sources are linked upstream")
                    .default_value(false)
                    .construct_only()
                    .build(),

                glib::ParamSpecBoolean::builder("ignore-inactive-pads")
                    .nick("Ignore inactive pads")
                    .blurb("Avoid timing out waiting for inactive pads")
                    .default_value(false)
                    .build(),

                glib::ParamSpecUInt64::builder("discont-wait")
                    .nick("Discont Wait")
                    .blurb("Window of time in nanoseconds to wait before creating a discontinuity")
                    .minimum(0u64)
                    .maximum(u64::MAX - 1)
                    .default_value(gst::ClockTime::from_mseconds(1000).nseconds())
                    .mutable_playing()
                    .build(),

                glib::ParamSpecUInt64::builder("alignment-threshold")
                    .nick("Alignment Threshold")
                    .blurb("Timestamp alignment threshold in nanoseconds")
                    .minimum(0u64)
                    .maximum(u64::MAX - 1)
                    .default_value(gst::ClockTime::from_mseconds(40).nseconds())
                    .mutable_playing()
                    .build(),

                gst::ParamSpecFraction::builder("output-buffer-duration-fraction")
                    .nick("Output buffer duration fraction")
                    .blurb("Output block size in nanoseconds, expressed as a fraction")
                    .minimum(gst::Fraction::new(1, i32::MAX))
                    .maximum(gst::Fraction::new(i32::MAX, 1))
                    .default_value(gst::Fraction::new(1, 100))
                    .mutable_ready()
                    .build(),

                glib::ParamSpecUInt64::builder("output-buffer-duration")
                    .nick("Output Buffer Duration")
                    .blurb("Output block size in nanoseconds")
                    .minimum(1u64)
                    .maximum(u64::MAX - 1)
                    .default_value(gst::ClockTime::from_mseconds(10).nseconds())
                    .mutable_ready()
                    .build(),

                // Aggregator properties we proxy for convenience and discoverability

                glib::ParamSpecUInt64::builder("start-time")
                    .nick("Start Time")
                    .blurb("Start time to use if start-time-selection=set")
                    .minimum(0u64)
                    .maximum(u64::MAX)
                    .default_value(u64::MAX)
                    .build(),

                glib::ParamSpecEnum::builder_with_default("start-time-selection", gst_base::AggregatorStartTimeSelection::Zero)
                    .nick("Start Time Selection")
                    .blurb("Decides which start time is output")
                    .build(),

                glib::ParamSpecUInt64::builder("min-upstream-latency")
                    .nick("Minimum upstream latency")
                    .blurb("When sources with a higher latency are expected to be plugged \
                            in dynamically after the aggregator has started playing, \
                            this allows overriding the minimum latency reported by the \
                            initial source(s). This is only taken into account when larger \
                            than the actually reported minimum latency. (nanoseconds)")
                    .minimum(0u64)
                    .maximum(u64::MAX)
                    .default_value(0u64)
                    .build(),

                glib::ParamSpecUInt64::builder("latency")
                    .nick("Buffer latency")
                    .blurb("Additional latency in live mode to allow upstream \
                            to take longer to produce buffers for the current \
                            position (in nanoseconds)")
                    .minimum(0u64)
                    .maximum(u64::MAX)
                    .default_value(0u64)
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

            "max-participants" => {
                let mut settings = self.settings.lock().unwrap();
                settings.max_participants = value.get::<u32>().unwrap();
            }

            "direction-model" | "air-absorption" | "rear-shadow" | "room" | "reverb-level"
            | "near-distance" => {
                let state_guard = self.state.lock().unwrap();
                let state = state_guard.as_ref().expect("state");
                let mut scene = state.scene.lock().unwrap();
                match pspec.name() {
                    "direction-model" => {
                        scene.acoustics.direction = value.get::<DirectionModel>().unwrap()
                    }
                    "room" => scene.acoustics.room = value.get::<crate::rooms::Room>().unwrap(),
                    "air-absorption" => scene.acoustics.air_absorption = value.get().unwrap(),
                    "reverb-level" => scene.acoustics.reverb_level = value.get().unwrap(),
                    "near-distance" => scene.acoustics.near_distance = value.get().unwrap(),
                    _ => scene.acoustics.rear_shadow = value.get().unwrap(),
                }
                scene.dirty = true;
            }

            name if SIZE_PROPERTIES.contains(&name)
                || MATERIAL_PROPERTIES.contains(&name)
                || ABSORPTION_PROPERTIES.contains(&name)
                || name == "room-contents" =>
            {
                let state_guard = self.state.lock().unwrap();
                let state = state_guard.as_ref().expect("state");
                let mut scene = state.scene.lock().unwrap();
                let dims = &mut scene.room_dimensions;
                if let Some(i) = SIZE_PROPERTIES.iter().position(|&n| n == name) {
                    dims.size[i] = value.get().unwrap();
                } else if let Some(i) = MATERIAL_PROPERTIES.iter().position(|&n| n == name) {
                    dims.materials[i] = value.get().unwrap();
                } else if let Some(i) = ABSORPTION_PROPERTIES.iter().position(|&n| n == name) {
                    dims.absorption[i] = numbers(value);
                } else {
                    dims.contents = value.get().unwrap();
                }
                scene.dirty = true;
            }

            name if ROOM_OVERRIDES.contains(&name) => {
                let state_guard = self.state.lock().unwrap();
                let state = state_guard.as_ref().expect("state");
                let mut scene = state.scene.lock().unwrap();
                let o = &mut scene.room_overrides;
                match name {
                    "reflection-delays" => o.delays_ms = numbers(value),
                    "reflection-levels" => o.levels_db = numbers(value),
                    "wall-cutoff" => o.cutoff_hz = optional(value),
                    "reverb-decay" => o.decay_s = optional(value),
                    "reverb-treble-decay" => o.treble_decay_s = optional(value),
                    "reverb-size" => o.size_ms = optional(value),
                    "room-falloff" => o.falloff = optional(value),
                    "reflection-spread" => o.spread = optional(value),
                    "reflection-diffusion" => o.diffusion = optional(value),
                    _ => o.near_floor = optional(value),
                }
                let o = o.clone();
                if !o.delays_ms.is_empty()
                    && !o.levels_db.is_empty()
                    && o.delays_ms.len() != o.levels_db.len()
                {
                    gst::debug!(
                        CAT,
                        imp = self,
                        "{} reflection delays and {} levels: using the first {}",
                        o.delays_ms.len(),
                        o.levels_db.len(),
                        o.delays_ms.len().min(o.levels_db.len())
                    );
                }
                scene.dirty = true;
            }

            "distance-model" | "reference-distance" | "rolloff" | "max-range" | "taper" => {
                let state_guard = self.state.lock().unwrap();
                let state = state_guard.as_ref().expect("state");
                let mut scene = state.scene.lock().unwrap();
                let params = &mut scene.params;
                match pspec.name() {
                    "distance-model" => params.model = value.get::<DistanceModel>().unwrap(),
                    "reference-distance" => params.reference = value.get().unwrap(),
                    "rolloff" => params.rolloff = value.get().unwrap(),
                    "taper" => params.taper = value.get().unwrap(),
                    _ => params.max_range = value.get().unwrap(),
                }
                scene.dirty = true;
            }

            "force-live" => {
                let mut settings = self.settings.lock().unwrap();
                settings.force_live = value.get::<bool>().unwrap();
            }

            "ignore-inactive-pads"
            | "discont-wait"
            | "alignment-threshold"
            | "output-buffer-duration-fraction"
            | "output-buffer-duration"
            | "start-time"
            | "start-time-selection"
            | "min-upstream-latency"
            | "latency" => {
                let state_guard = self.state.lock().unwrap();
                let state = state_guard.as_ref().expect("state");

                state.mixer_element.set_property(pspec.name(), value);
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

            "max-participants" => {
                let settings = self.settings.lock().unwrap();
                settings.max_participants.to_value()
            }

            "direction-model" | "air-absorption" | "rear-shadow" | "room" | "reverb-level"
            | "near-distance" => {
                let state_guard = self.state.lock().unwrap();
                let state = state_guard.as_ref().expect("state");
                let acoustics = state.scene.lock().unwrap().acoustics;
                match pspec.name() {
                    "direction-model" => acoustics.direction.to_value(),
                    "room" => acoustics.room.to_value(),
                    "air-absorption" => acoustics.air_absorption.to_value(),
                    "reverb-level" => acoustics.reverb_level.to_value(),
                    "near-distance" => acoustics.near_distance.to_value(),
                    _ => acoustics.rear_shadow.to_value(),
                }
            }

            name if ROOM_OVERRIDES.contains(&name) => {
                let state_guard = self.state.lock().unwrap();
                let state = state_guard.as_ref().expect("state");
                let o = state.scene.lock().unwrap().room_overrides.clone();
                let number = |v: Option<f64>| v.unwrap_or(-1.0).to_value();
                match name {
                    "reflection-delays" => gst::Array::new(o.delays_ms).to_value(),
                    "reflection-levels" => gst::Array::new(o.levels_db).to_value(),
                    "wall-cutoff" => number(o.cutoff_hz),
                    "reverb-decay" => number(o.decay_s),
                    "reverb-treble-decay" => number(o.treble_decay_s),
                    "reverb-size" => number(o.size_ms),
                    "room-falloff" => number(o.falloff),
                    "reflection-spread" => number(o.spread),
                    "reflection-diffusion" => number(o.diffusion),
                    _ => number(o.near_floor),
                }
            }

            name if SIZE_PROPERTIES.contains(&name)
                || MATERIAL_PROPERTIES.contains(&name)
                || ABSORPTION_PROPERTIES.contains(&name)
                || name == "room-contents" =>
            {
                let state_guard = self.state.lock().unwrap();
                let state = state_guard.as_ref().expect("state");
                let dims = state.scene.lock().unwrap().room_dimensions.clone();
                if let Some(i) = SIZE_PROPERTIES.iter().position(|&n| n == name) {
                    dims.size[i].to_value()
                } else if let Some(i) = MATERIAL_PROPERTIES.iter().position(|&n| n == name) {
                    dims.materials[i].to_value()
                } else if let Some(i) = ABSORPTION_PROPERTIES.iter().position(|&n| n == name) {
                    gst::Array::new(dims.absorption[i].clone()).to_value()
                } else {
                    dims.contents.to_value()
                }
            }

            "room-parameters" => {
                let state_guard = self.state.lock().unwrap();
                let state = state_guard.as_ref().expect("state");
                let scene = state.scene.lock().unwrap();
                room_parameters(&scene).to_value()
            }

            "distance-model" | "reference-distance" | "rolloff" | "max-range" | "taper" => {
                let state_guard = self.state.lock().unwrap();
                let state = state_guard.as_ref().expect("state");
                let params = state.scene.lock().unwrap().params;
                match pspec.name() {
                    "distance-model" => params.model.to_value(),
                    "reference-distance" => params.reference.to_value(),
                    "rolloff" => params.rolloff.to_value(),
                    "taper" => params.taper.to_value(),
                    _ => params.max_range.to_value(),
                }
            }

            "force-live" => {
                let settings = self.settings.lock().unwrap();
                settings.force_live.to_value()
            }

            "ignore-inactive-pads"
            | "discont-wait"
            | "alignment-threshold"
            | "output-buffer-duration-fraction"
            | "output-buffer-duration"
            | "start-time"
            | "start-time-selection"
            | "min-upstream-latency"
            | "latency" => {
                let state_guard = self.state.lock().unwrap();
                let state = state_guard.as_ref().expect("state");

                state.mixer_element.property(pspec.name())
            }

            name => unimplemented!("{name}"),
        }
    }
}

/// `room-parameters`: the room's values in effect (the reverb's only when it has one; `wall-cutoff` 0 = none), the
/// names of the properties overriding its preset (`overridden`) and of those its dimensions give (`derived`), and,
/// with dimensions, the surfaces' absorption in effect (bass, treble).
fn room_parameters(scene: &Scene) -> gst::Structure {
    let room = scene.room();
    let overridden = scene.room_overrides.names();
    let dims = &scene.room_dimensions;
    let derived: Vec<&str> = if dims.given() {
        DERIVED_PROPERTIES
            .iter()
            .copied()
            .filter(|n| !overridden.contains(n))
            .collect()
    } else {
        vec![]
    };
    let mut s = gst::Structure::builder("room-parameters")
        .field("room", scene.acoustics.room)
        .field(
            "reflection-delays",
            gst::Array::new(room.taps.iter().map(|t| t.delay_ms)),
        )
        .field(
            "reflection-levels",
            gst::Array::new(room.taps.iter().map(|t| t.gain_db)),
        )
        .field(
            "wall-cutoff",
            if room.cutoff_hz.is_finite() {
                room.cutoff_hz
            } else {
                0.0
            },
        )
        .field("room-falloff", room.falloff)
        .field("reflection-spread", room.spread)
        .field("reflection-diffusion", room.diffusion)
        .field("near-floor", room.near_floor)
        .field("overridden", gst::Array::new(overridden))
        .field("derived", gst::Array::new(derived))
        .build();
    if let Some(reverb) = room.reverb {
        s.set("reverb-decay", reverb.decay_s);
        s.set("reverb-treble-decay", reverb.treble_decay_s);
        s.set("reverb-size", reverb.size_ms);
        // The room's own reverb level, which reverb-level adjusts (dB against a voice at the reference distance).
        s.set("reverb-room-level", reverb.level_db);
    }
    if dims.given() {
        for (i, name) in ABSORPTION_PROPERTIES.iter().enumerate() {
            s.set(*name, gst::Array::new(dims.absorption(i)));
        }
    }
    s
}

impl GstObjectImpl for AcousticMultiMixer {}

impl ElementImpl for AcousticMultiMixer {
    fn metadata() -> Option<&'static gst::subclass::ElementMetadata> {
        static ELEMENT_METADATA: LazyLock<gst::subclass::ElementMetadata> = LazyLock::new(|| {
            gst::subclass::ElementMetadata::new(
                "Acoustic Multi Mixer",
                "Audio/Mixer",
                "Mix-minus mixer with positions: every participant hears everyone else in stereo, by distance and direction",
                "Simon Annetts/Jak Daniels, Tim-Philipp Müller <tim@centricular.com>",
            )
        });

        Some(&*ELEMENT_METADATA)
    }

    fn pad_templates() -> &'static [gst::PadTemplate] {
        static PAD_TEMPLATES: LazyLock<Vec<gst::PadTemplate>> = LazyLock::new(|| {
            let caps = gst_audio::AudioCapsBuilder::new_interleaved()
                .format_list([gst_audio::AUDIO_FORMAT_S16, gst_audio::AUDIO_FORMAT_F32])
                .rate_range(1..)
                .channels(1) // mono voices
                .build();

            let sink_pad_template = gst::PadTemplate::with_gtype(
                "sink_%u",
                gst::PadDirection::Sink,
                gst::PadPresence::Request,
                &caps,
                crate::AcousticMultiMixerPad::static_type(),
            )
            .unwrap();

            let stereo_caps = gst_audio::AudioCapsBuilder::new_interleaved()
                .format_list([gst_audio::AUDIO_FORMAT_S16, gst_audio::AUDIO_FORMAT_F32])
                .rate_range(1..)
                .channels(2)
                .build();

            let src_pad_template = gst::PadTemplate::new(
                "mix_%u",
                gst::PadDirection::Src,
                gst::PadPresence::Sometimes,
                &stereo_caps,
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
        caps: Option<&gst::Caps>,
    ) -> Option<gst::Pad> {
        let mut state_guard = self.state.lock().unwrap();
        let state = state_guard.as_mut().expect("state");

        // Pad numbers index the fixed outputs: a free one below max-participants.
        let n = match name.and_then(|name| name.strip_prefix("sink_")) {
            Some("%u") | None => (0..state.max_participants).find(|n| !state.streams.contains(n)),
            Some(suffix) => suffix
                .parse::<u32>()
                .ok()
                .filter(|n| *n < state.max_participants && !state.streams.contains(n)),
        };
        let Some(n) = n else {
            gst::error!(
                CAT,
                imp = self,
                "No pad {} available (max-participants {}, in use {:?})",
                name.unwrap_or("sink_%u"),
                state.max_participants,
                state.streams
            );
            return None;
        };

        let mixer_templ = state.mixer_element.pad_template("sink_%u").unwrap();

        let mixer_pad =
            state
                .mixer_element
                .request_pad(&mixer_templ, Some(&format!("sink_{n}")), caps)?;

        let new_sink_pad = gst::PadBuilder::<crate::AcousticMultiMixerPad>::from_template(templ)
            .name(format!("sink_{n}"))
            .with_target(&mixer_pad)
            .unwrap()
            .build();
        new_sink_pad.imp().attach(state.scene.clone(), n as usize);

        self.obj().add_pad(&new_sink_pad).unwrap();
        new_sink_pad.set_active(true).unwrap();

        let splitter_templ = state.splitter.pad_template("src_%u").unwrap();

        let splitter_pad =
            state
                .splitter
                .request_pad(&splitter_templ, Some(&format!("src_{n}")), caps)?;

        let src_templ = self.obj().pad_template("mix_%u").unwrap();

        let mix_pad = gst::GhostPad::builder_from_template_with_target(&src_templ, &splitter_pad)
            .unwrap()
            .name(format!("mix_{n}"))
            .build();
        // Only linked outputs are mixed (an unlinked one is a participant who only talks).
        let scene = state.scene.clone();
        mix_pad.connect_linked(move |_, _| {
            scene.lock().unwrap().set_listening(n as usize, true);
        });
        let scene = state.scene.clone();
        mix_pad.connect_unlinked(move |_, _| {
            scene.lock().unwrap().set_listening(n as usize, false);
        });

        self.obj().add_pad(&mix_pad).unwrap();
        mix_pad.set_active(true).unwrap();

        gst::info!(
            CAT,
            imp = self,
            "New input {} -> {}",
            &mixer_pad.name(),
            &mix_pad.name(),
        );

        state.streams.push(n);
        state.scene.lock().unwrap().join(n as usize);
        drop(state_guard);

        self.obj().child_added(&new_sink_pad, &new_sink_pad.name());

        Some(new_sink_pad.upcast())
    }

    fn release_pad(&self, pad: &gst::Pad) {
        let mut state_guard = self.state.lock().unwrap();
        let state = state_guard.as_mut().expect("state");

        if pad.direction() != gst::PadDirection::Sink {
            gst::error!(
                CAT,
                imp = self,
                "You must release the sink pad(s), in which case the\
                corresponding source pad will be removed automatically!"
            );
            return;
        }

        let n = pad
            .name()
            .strip_prefix("sink_")
            .unwrap()
            .parse::<u32>()
            .unwrap();

        let ghost_pad = pad.downcast_ref::<gst::GhostPad>().unwrap();
        if let Some(mixer_sink_pad) = ghost_pad.target() {
            state.mixer_element.release_request_pad(&mixer_sink_pad);
        }

        // Remove automatically-added mix_X sometimes source pad
        let mix_name = format!("mix_{n}");
        let mix_pad = self.obj().static_pad(&mix_name).unwrap();

        let src_ghost_pad = mix_pad.downcast_ref::<gst::GhostPad>().unwrap();
        if let Some(splitter_src_pad) = src_ghost_pad.target() {
            // ... but first release the splitter source pad so the splitter can make sure that
            // the pad removal and deactivation doesn't cause flow-flushing returns anywhere.
            state.splitter.release_request_pad(&splitter_src_pad);
        }

        mix_pad.set_active(false).unwrap();
        self.obj().remove_pad(&mix_pad).unwrap();

        pad.set_active(false).unwrap();
        self.obj().remove_pad(pad).unwrap();

        gst::info!(CAT, imp = self, "Released input sink_{n} -> {mix_name}");

        // Wasteful since we continue after we found the value, but we have small lengths
        state.streams.retain(|&val| val != n);
        state.scene.lock().unwrap().leave(n as usize);
        drop(state_guard);

        self.obj().child_removed(pad, &pad.name());
    }
}
