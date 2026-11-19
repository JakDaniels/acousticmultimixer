// Copyright 2026 Simon Annetts/Jak Daniels
// SPDX-License-Identifier: MPL-2.0

//! Room presets: the early reflections every voice gets in a kind of space, and its late reverb. The reflections
//! are a few taps (a delay after the direct sound and a level) and a treble cut for what the walls absorb; the
//! reverb is a decay time (in the bass and in the treble), a level and a size. Data only, so a new room needs no
//! code.

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
    /// The decaying tail after them; none outdoors.
    pub reverb: Option<Reverb>,
}

/// A room's late reverb: a feedback delay network per voice (see `dsp`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Reverb {
    /// Time to fall by 60 dB (s): in the bass, and in the treble (at the top of the band; shorter, as walls and
    /// air absorb more of it).
    pub decay_s: f64,
    pub treble_decay_s: f64,
    /// The tail's energy against the direct sound of a voice at the reference distance (dB, each ear).
    pub level_db: f64,
    /// The delay lines' mean length (ms): about how long sound takes between walls, so it also sets when the tail
    /// starts.
    pub size_ms: f64,
}

const fn tap(delay_ms: f64, gain_db: f64) -> Tap {
    Tap { delay_ms, gain_db }
}

const TINY_ROOM: &[Tap] = &[
    tap(1.5, -8.0),
    tap(3.5, -11.0),
    tap(5.5, -14.0),
    tap(8.0, -18.0),
];
const SMALL_ROOM: &[Tap] = &[
    tap(3.0, -9.0),
    tap(7.0, -12.0),
    tap(11.0, -15.0),
    tap(17.0, -19.0),
];
const BIG_ROOM: &[Tap] = &[
    tap(8.0, -9.0),
    tap(15.0, -12.0),
    tap(24.0, -15.0),
    tap(36.0, -19.0),
];
const HALL: &[Tap] = &[
    tap(15.0, -10.0),
    tap(25.0, -12.0),
    tap(40.0, -15.0),
    tap(70.0, -19.0),
];
/// Close side walls first, then the far ends.
const LONG_HALL: &[Tap] = &[
    tap(5.0, -9.0),
    tap(12.0, -12.0),
    tap(45.0, -16.0),
    tap(85.0, -20.0),
];
const CAVE: &[Tap] = &[
    tap(25.0, -8.0),
    tap(45.0, -12.0),
    tap(70.0, -15.0),
    tap(110.0, -20.0),
];
const CATHEDRAL: &[Tap] = &[
    tap(20.0, -10.0),
    tap(38.0, -13.0),
    tap(60.0, -16.0),
    tap(90.0, -20.0),
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

    #[enum_value(
        name = "Tiny room: a closet or bathroom, very close walls",
        nick = "tiny-room"
    )]
    Tiny,

    #[enum_value(name = "Small room: short, close reflections", nick = "small-room")]
    Small,

    #[enum_value(name = "Big room: a classroom or large office", nick = "big-room")]
    Big,

    #[enum_value(name = "Hall: later, more spread-out reflections", nick = "hall")]
    Hall,

    #[enum_value(
        name = "Long hall: a gallery or corridor, near sides and far ends",
        nick = "long-hall"
    )]
    LongHall,

    #[enum_value(name = "Cave: a large rocky chamber, dark and long", nick = "cave")]
    Cave,

    #[enum_value(
        name = "Cathedral: vast stone and glass, a long bright tail",
        nick = "cathedral"
    )]
    Cathedral,
}

impl Room {
    pub fn preset(self) -> Preset {
        match self {
            Room::Outdoor => Preset {
                taps: &[],
                cutoff_hz: f64::INFINITY,
                reverb: None,
            },
            Room::Tiny => Preset {
                taps: TINY_ROOM,
                cutoff_hz: 7000.0,
                reverb: Some(Reverb {
                    decay_s: 0.3,
                    treble_decay_s: 0.12,
                    level_db: -16.0,
                    size_ms: 6.0,
                }),
            },
            Room::Small => Preset {
                taps: SMALL_ROOM,
                cutoff_hz: 6000.0,
                reverb: Some(Reverb {
                    decay_s: 0.4,
                    treble_decay_s: 0.15,
                    level_db: -15.0,
                    size_ms: 12.0,
                }),
            },
            Room::Big => Preset {
                taps: BIG_ROOM,
                cutoff_hz: 5500.0,
                reverb: Some(Reverb {
                    decay_s: 0.7,
                    treble_decay_s: 0.25,
                    level_db: -15.0,
                    size_ms: 25.0,
                }),
            },
            Room::Hall => Preset {
                taps: HALL,
                cutoff_hz: 5000.0,
                reverb: Some(Reverb {
                    decay_s: 1.1,
                    treble_decay_s: 0.35,
                    level_db: -15.0,
                    size_ms: 45.0,
                }),
            },
            Room::LongHall => Preset {
                taps: LONG_HALL,
                cutoff_hz: 5000.0,
                reverb: Some(Reverb {
                    decay_s: 1.4,
                    treble_decay_s: 0.4,
                    level_db: -15.0,
                    size_ms: 40.0,
                }),
            },
            Room::Cave => Preset {
                taps: CAVE,
                cutoff_hz: 3000.0,
                reverb: Some(Reverb {
                    decay_s: 1.8,
                    treble_decay_s: 0.5,
                    level_db: -13.0,
                    size_ms: 70.0,
                }),
            },
            Room::Cathedral => Preset {
                taps: CATHEDRAL,
                cutoff_hz: 4000.0,
                reverb: Some(Reverb {
                    decay_s: 3.0,
                    treble_decay_s: 1.2,
                    level_db: -12.0,
                    size_ms: 90.0,
                }),
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
