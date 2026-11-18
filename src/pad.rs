// Copyright 2026 Simon Annetts/Jak Daniels
// SPDX-License-Identifier: MPL-2.0

//! The element's `sink_%u` pads: one per participant, carrying where they are, which way they face, where they
//! listen from, their gain and mute, and their own gains for others (`trims`). The properties read and write the
//! shared scene directly.

use gst::glib;
use gst::prelude::*;
use gst::subclass::prelude::*;

use std::sync::{Arc, LazyLock, Mutex, OnceLock};

use crate::scene::{MAX_GAIN, Participant, Scene};

#[derive(Default)]
pub struct SinkPad {
    // The scene and this pad's participant number, set when the element creates the pad.
    slot: OnceLock<(Arc<Mutex<Scene>>, usize)>,
}

impl SinkPad {
    pub(crate) fn attach(&self, scene: Arc<Mutex<Scene>>, n: usize) {
        self.slot
            .set((scene, n))
            .unwrap_or_else(|_| panic!("pad attached twice"));
    }

    fn with<R>(&self, f: impl FnOnce(&mut Participant) -> R, changes: bool) -> Option<R> {
        let (scene, n) = self.slot.get()?;
        let mut scene = scene.lock().unwrap();
        let r = f(&mut scene.participants[*n]);
        if changes {
            scene.dirty = true;
        }
        Some(r)
    }
}

#[glib::object_subclass]
impl ObjectSubclass for SinkPad {
    const NAME: &'static str = "GstAcousticMultiMixerPad";
    type Type = super::AcousticMultiMixerPad;
    type ParentType = gst::GhostPad;
}

fn coordinate(name: &str, blurb: &str) -> glib::ParamSpec {
    glib::ParamSpecDouble::builder(name)
        .nick(name)
        .blurb(blurb)
        .minimum(-1e9)
        .maximum(1e9)
        .default_value(0.0)
        .mutable_playing()
        .build()
}

fn angle(name: &str, blurb: &str) -> glib::ParamSpec {
    glib::ParamSpecDouble::builder(name)
        .nick(name)
        .blurb(blurb)
        .minimum(-360.0)
        .maximum(360.0)
        .default_value(0.0)
        .mutable_playing()
        .build()
}

impl ObjectImpl for SinkPad {
    fn properties() -> &'static [glib::ParamSpec] {
        static PROPERTIES: LazyLock<Vec<glib::ParamSpec>> = LazyLock::new(|| {
            vec![
                coordinate("x", "Where the voice is: east (sets positioned)"),
                coordinate("y", "Where the voice is: north (sets positioned)"),
                coordinate("z", "Where the voice is: up (sets positioned)"),
                angle(
                    "yaw",
                    "Facing, degrees, 0 = +x, turning left (sets positioned; no effect on the voice yet)",
                ),
                angle(
                    "pitch",
                    "Looking up, degrees (sets positioned; no effect on the voice yet)",
                ),
                angle(
                    "roll",
                    "Right ear down, degrees (sets positioned; no effect on the voice yet)",
                ),
                glib::ParamSpecBoolean::builder("positioned")
                    .nick("Positioned")
                    .blurb(
                        "Has a position; without one the voice is heard centred at full volume, \
                         and hears everyone that way",
                    )
                    .default_value(false)
                    .mutable_playing()
                    .build(),
                coordinate(
                    "listener-x",
                    "Where this participant hears from: east (sets listener-separate)",
                ),
                coordinate(
                    "listener-y",
                    "Where this participant hears from: north (sets listener-separate)",
                ),
                coordinate(
                    "listener-z",
                    "Where this participant hears from: up (sets listener-separate)",
                ),
                angle(
                    "listener-yaw",
                    "Listening facing, degrees (sets listener-separate)",
                ),
                angle(
                    "listener-pitch",
                    "Listening pitch, degrees (sets listener-separate)",
                ),
                angle(
                    "listener-roll",
                    "Listening roll, degrees (sets listener-separate)",
                ),
                glib::ParamSpecBoolean::builder("listener-separate")
                    .nick("Separate listener pose")
                    .blurb(
                        "Hear from the listener-* pose; otherwise from the speaking pose \
                         (x, y, z, yaw, pitch, roll)",
                    )
                    .default_value(false)
                    .mutable_playing()
                    .build(),
                glib::ParamSpecDouble::builder("gain")
                    .nick("Gain")
                    .blurb("Gain of this voice for everyone hearing it")
                    .minimum(0.0)
                    .maximum(MAX_GAIN)
                    .default_value(1.0)
                    .mutable_playing()
                    .build(),
                glib::ParamSpecBoolean::builder("mute")
                    .nick("Mute")
                    .blurb("Nobody hears this voice")
                    .default_value(false)
                    .mutable_playing()
                    .build(),
                glib::ParamSpecBoxed::builder::<gst::Structure>("trims")
                    .nick("Trims")
                    .blurb(
                        "What this participant has done to others' voices, as gains by speaker pad \
                         (0 = muted for them only, up to 10), e.g. trims,sink_0=0.0,sink_2=0.5",
                    )
                    .mutable_playing()
                    .build(),
            ]
        });

        PROPERTIES.as_ref()
    }

    fn set_property(&self, _id: usize, value: &glib::Value, pspec: &glib::ParamSpec) {
        let name = pspec.name();
        if name == "trims" {
            let trims = value
                .get::<Option<gst::Structure>>()
                .unwrap()
                .map(|s| parse_trims(&s))
                .unwrap_or_default();
            self.with(|p| p.trims = trims, true);
            return;
        }
        if let Ok(b) = value.get::<bool>() {
            self.with(
                |p| match name {
                    "positioned" => p.positioned = b,
                    "listener-separate" => p.listener_separate = b,
                    "mute" => p.mute = b,
                    _ => unimplemented!("{name}"),
                },
                true,
            );
            return;
        }
        let v = value.get::<f64>().unwrap();
        self.with(
            |p| match name {
                "x" | "y" | "z" | "yaw" | "pitch" | "roll" => {
                    p.positioned = true;
                    match name {
                        "x" => p.position[0] = v,
                        "y" => p.position[1] = v,
                        "z" => p.position[2] = v,
                        "yaw" => p.orientation.yaw = v,
                        "pitch" => p.orientation.pitch = v,
                        _ => p.orientation.roll = v,
                    }
                }
                "gain" => p.gain = v,
                _ => {
                    p.listener_separate = true;
                    match name {
                        "listener-x" => p.listener_position[0] = v,
                        "listener-y" => p.listener_position[1] = v,
                        "listener-z" => p.listener_position[2] = v,
                        "listener-yaw" => p.listener_orientation.yaw = v,
                        "listener-pitch" => p.listener_orientation.pitch = v,
                        "listener-roll" => p.listener_orientation.roll = v,
                        _ => unimplemented!("{name}"),
                    }
                }
            },
            true,
        );
    }

    fn property(&self, _id: usize, pspec: &glib::ParamSpec) -> glib::Value {
        let name = pspec.name();
        let default = Participant::default();
        let get = |p: &mut Participant| -> glib::Value {
            match name {
                "x" => p.position[0].to_value(),
                "y" => p.position[1].to_value(),
                "z" => p.position[2].to_value(),
                "yaw" => p.orientation.yaw.to_value(),
                "pitch" => p.orientation.pitch.to_value(),
                "roll" => p.orientation.roll.to_value(),
                "positioned" => p.positioned.to_value(),
                "listener-x" => p.listener_position[0].to_value(),
                "listener-y" => p.listener_position[1].to_value(),
                "listener-z" => p.listener_position[2].to_value(),
                "listener-yaw" => p.listener_orientation.yaw.to_value(),
                "listener-pitch" => p.listener_orientation.pitch.to_value(),
                "listener-roll" => p.listener_orientation.roll.to_value(),
                "listener-separate" => p.listener_separate.to_value(),
                "gain" => p.gain.to_value(),
                "mute" => p.mute.to_value(),
                "trims" => {
                    let mut s = gst::Structure::new_empty("trims");
                    for (n, g) in &p.trims {
                        s.set(format!("sink_{n}"), *g);
                    }
                    s.to_value()
                }
                _ => unimplemented!("{name}"),
            }
        };
        self.with(get, false)
            .unwrap_or_else(|| get(&mut default.clone()))
    }
}

/// `trims,sink_0=0.0,sink_2=0.5` → {0: 0.0, 2: 0.5}. Fields that are not `sink_N` with a number are ignored.
fn parse_trims(s: &gst::StructureRef) -> std::collections::BTreeMap<u32, f64> {
    s.iter()
        .filter_map(|(field, value)| {
            let n = field.strip_prefix("sink_")?.parse::<u32>().ok()?;
            let g = value
                .get::<f64>()
                .ok()
                .or_else(|| value.get::<f32>().ok().map(f64::from))
                .or_else(|| value.get::<i32>().ok().map(f64::from))?;
            Some((n, g.clamp(0.0, MAX_GAIN)))
        })
        .collect()
}

impl GstObjectImpl for SinkPad {}

impl PadImpl for SinkPad {}

impl ProxyPadImpl for SinkPad {}

impl GhostPadImpl for SinkPad {}
