// Copyright 2026 Simon Annetts/Jak Daniels
// SPDX-License-Identifier: MPL-2.0

//! Room presets: the early reflections every voice gets in a kind of space. Each is a few taps (a delay after the
//! direct sound and a level), and a treble cut for what the walls absorb. Data only, so a new room needs no code.

use gst::glib;

/// One early reflection: arriving `delay_ms` after the direct sound, `gain_db` below a voice heard at the
/// reference distance.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tap {
    pub delay_ms: f64,
    pub gain_db: f64,
}

/// A kind of space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Preset {
    /// Early reflections, earliest first; later ones are heard less from the voice's direction and more from all
    /// around.
    pub taps: &'static [Tap],
    /// One-pole low-pass on the reflections (Hz): the walls' treble absorption.
    pub cutoff_hz: f64,
}

const fn tap(delay_ms: f64, gain_db: f64) -> Tap {
    Tap { delay_ms, gain_db }
}

const SMALL_ROOM: &[Tap] = &[
    tap(3.0, -9.0),
    tap(7.0, -12.0),
    tap(11.0, -15.0),
    tap(17.0, -19.0),
];
const HALL: &[Tap] = &[
    tap(15.0, -10.0),
    tap(25.0, -12.0),
    tap(40.0, -15.0),
    tap(70.0, -19.0),
];
const CAVE: &[Tap] = &[
    tap(80.0, -8.0),
    tap(140.0, -12.0),
    tap(220.0, -15.0),
    tap(430.0, -20.0),
];

/// The room a scene is in.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, glib::Enum)]
#[repr(u32)]
#[enum_type(name = "GstAcousticMultiMixerRoom")]
pub enum Room {
    /// Open air: no reflections, the direct sound only.
    #[default]
    #[enum_value(name = "Outdoor: no reflections", nick = "outdoor")]
    Outdoor,

    #[enum_value(name = "Small room: short, close reflections", nick = "small-room")]
    Small,

    #[enum_value(name = "Hall: later, more spread-out reflections", nick = "hall")]
    Hall,

    #[enum_value(name = "Cave: long, strong echoes", nick = "cave")]
    Cave,
}

impl Room {
    pub fn preset(self) -> Preset {
        match self {
            Room::Outdoor => Preset {
                taps: &[],
                cutoff_hz: f64::INFINITY,
            },
            Room::Small => Preset {
                taps: SMALL_ROOM,
                cutoff_hz: 6000.0,
            },
            Room::Hall => Preset {
                taps: HALL,
                cutoff_hz: 5000.0,
            },
            Room::Cave => Preset {
                taps: CAVE,
                cutoff_hz: 3000.0,
            },
        }
    }
}

/// How far a tap's left and right copies are moved apart (a fraction of its delay, alternately earlier on the left
/// and on the right), so that reflections from all around are not identical in both ears.
pub const SPREAD: f64 = 0.08;

/// Tap k's share (of n) heard from all around rather than from the voice's direction: rising to all of it for the
/// last tap.
pub fn diffuse_share(k: usize, n: usize) -> f64 {
    (k + 1) as f64 / n as f64
}
