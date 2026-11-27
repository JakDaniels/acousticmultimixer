// Copyright 2026 Simon Annetts/Jak Daniels
// SPDX-License-Identifier: MPL-2.0

//! Room presets: the early reflections every voice gets in a kind of space, and its late reverb. The reflections
//! are a few taps (a delay after the direct sound and a level) and a treble cut for what the walls absorb; the
//! reverb is a decay time (in the bass and in the treble), a level and a size. Data only, so a new room needs no
//! code.
//!
//! Each of a preset's values can be overridden (the element's room properties, `RoomOverrides`), and the values in
//! effect (`RoomParams`) are what the mixer uses.

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

/// How fast a voice's reflections and reverb fall with distance, against its direct sound (in dB). Half (as at
/// first) left distant voices too loud right up to the edge of the range.
pub const ROOM_FALLOFF: f64 = 0.75;

/// Close up the direct sound stays at full volume (within the reference distance, so nearby voices are not too
/// loud), so instead the room is turned down closer than the near distance: 6 dB per halving of the distance, down
/// to this distance (m), as a voice right next to you stands out from the room.
pub const NEAR_FLOOR: f64 = 0.5;

/// The most taps a room may have.
pub const MAX_TAPS: usize = 16;
/// The latest a tap may arrive (ms after the direct sound).
pub const MAX_TAP_DELAY_MS: f64 = 500.0;
/// Limits on the reverb's times, so a value at the end of a property's range still makes a working network.
pub const MIN_DECAY_S: f64 = 0.05;
pub const MIN_SIZE_MS: f64 = 1.0;

/// The values a room is mixed with: its preset's, with any overrides.
#[derive(Debug, Clone, PartialEq)]
pub struct RoomParams {
    /// Early reflections, earliest first.
    pub taps: Vec<Tap>,
    /// The reflections' low-pass (Hz; infinite = none).
    pub cutoff_hz: f64,
    pub reverb: Option<Reverb>,
    /// How fast the room's sound falls with distance against the direct sound (a power of the direct gain).
    pub falloff: f64,
    /// The diffuse copies' offset, a fraction of each tap's delay.
    pub spread: f64,
    /// The first tap's share heard from all around, rising evenly to all of it for the last.
    pub diffusion: f64,
    /// Where `near-distance`'s turn-down stops (m).
    pub near_floor: f64,
}

impl Default for RoomParams {
    fn default() -> Self {
        Room::Outdoor.params(&RoomOverrides::default())
    }
}

impl RoomParams {
    /// Tap k's share heard from all around rather than from the voice's direction: `diffusion` for the first,
    /// rising evenly to all of it for the last (a single tap: all of it).
    pub fn diffuse_share(&self, k: usize) -> f64 {
        let n = self.taps.len();
        if n <= 1 {
            1.0
        } else {
            self.diffusion + (1.0 - self.diffusion) * k as f64 / (n - 1) as f64
        }
    }

    /// Whether the room adds anything to the direct sound.
    pub fn reflects(&self) -> bool {
        !self.taps.is_empty() || self.reverb.is_some()
    }
}

/// What a room's properties have changed from its preset (None, or empty lists: the preset's).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RoomOverrides {
    pub delays_ms: Vec<f64>,
    pub levels_db: Vec<f64>,
    pub cutoff_hz: Option<f64>,
    pub decay_s: Option<f64>,
    pub treble_decay_s: Option<f64>,
    pub size_ms: Option<f64>,
    pub falloff: Option<f64>,
    pub spread: Option<f64>,
    pub diffusion: Option<f64>,
    pub near_floor: Option<f64>,
}

impl RoomOverrides {
    /// The element properties that are set, for `room-parameters`.
    pub fn names(&self) -> Vec<&'static str> {
        let mut names = vec![];
        if !self.delays_ms.is_empty() {
            names.push("reflection-delays");
        }
        if !self.levels_db.is_empty() {
            names.push("reflection-levels");
        }
        for (name, value) in [
            ("wall-cutoff", self.cutoff_hz),
            ("reverb-decay", self.decay_s),
            ("reverb-treble-decay", self.treble_decay_s),
            ("reverb-size", self.size_ms),
            ("room-falloff", self.falloff),
            ("reflection-spread", self.spread),
            ("reflection-diffusion", self.diffusion),
            ("near-floor", self.near_floor),
        ] {
            if value.is_some() {
                names.push(name);
            }
        }
        names
    }
}

impl Room {
    /// The values in effect: the preset's, with the overrides. Outdoors there is no room, so overrides of its
    /// reflections and reverb have no effect (start from an interior preset instead). Tap delays and levels pair up
    /// in order (as many taps as the shorter list), sorted by delay; values are kept within working limits.
    pub fn params(self, o: &RoomOverrides) -> RoomParams {
        let preset = self.preset();
        let taps = if self == Room::Outdoor {
            vec![]
        } else {
            let delays: Vec<f64> = if o.delays_ms.is_empty() {
                preset.taps.iter().map(|t| t.delay_ms).collect()
            } else {
                o.delays_ms.clone()
            };
            let levels: Vec<f64> = if o.levels_db.is_empty() {
                preset.taps.iter().map(|t| t.gain_db).collect()
            } else {
                o.levels_db.clone()
            };
            let mut taps: Vec<Tap> = delays
                .iter()
                .zip(&levels)
                .take(MAX_TAPS)
                .map(|(&d, &g)| tap(d.clamp(0.0, MAX_TAP_DELAY_MS), g))
                .collect();
            taps.sort_by(|a, b| a.delay_ms.total_cmp(&b.delay_ms));
            taps
        };
        let cutoff_hz = match o.cutoff_hz {
            Some(c) if self != Room::Outdoor => {
                if c <= 0.0 {
                    f64::INFINITY
                } else {
                    c
                }
            }
            _ => preset.cutoff_hz,
        };
        let reverb = preset.reverb.map(|r| {
            let decay_s = o.decay_s.unwrap_or(r.decay_s).max(MIN_DECAY_S);
            Reverb {
                decay_s,
                treble_decay_s: o
                    .treble_decay_s
                    .unwrap_or(r.treble_decay_s)
                    .clamp(MIN_DECAY_S, decay_s),
                level_db: r.level_db,
                size_ms: o.size_ms.unwrap_or(r.size_ms).max(MIN_SIZE_MS),
            }
        });
        RoomParams {
            diffusion: o
                .diffusion
                .unwrap_or(1.0 / taps.len().max(1) as f64)
                .clamp(0.0, 1.0),
            taps,
            cutoff_hz,
            reverb,
            falloff: o.falloff.unwrap_or(ROOM_FALLOFF),
            spread: o.spread.unwrap_or(SPREAD),
            near_floor: o.near_floor.unwrap_or(NEAR_FLOOR),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn delays(p: &RoomParams) -> Vec<f64> {
        p.taps.iter().map(|t| t.delay_ms).collect()
    }

    #[test]
    fn without_overrides_a_room_is_its_preset() {
        let hall = Room::Hall.params(&RoomOverrides::default());
        let preset = Room::Hall.preset();
        assert_eq!(hall.taps, preset.taps);
        assert_eq!(hall.cutoff_hz, preset.cutoff_hz);
        assert_eq!(hall.reverb, preset.reverb);
        assert_eq!(
            (hall.falloff, hall.spread, hall.near_floor),
            (ROOM_FALLOFF, SPREAD, NEAR_FLOOR)
        );
        // The shares are the presets' (k + 1) / n.
        for k in 0..4 {
            assert!((hall.diffuse_share(k) - (k + 1) as f64 / 4.0).abs() < 1e-12);
        }
    }

    #[test]
    fn overrides_replace_single_values() {
        let o = RoomOverrides {
            decay_s: Some(2.0),
            cutoff_hz: Some(0.0),
            diffusion: Some(0.0),
            ..RoomOverrides::default()
        };
        let hall = Room::Hall.params(&o);
        let reverb = hall.reverb.unwrap();
        assert_eq!(reverb.decay_s, 2.0);
        assert_eq!(reverb.treble_decay_s, 0.35); // still the preset's
        assert_eq!(hall.cutoff_hz, f64::INFINITY); // 0 = no filter
        assert_eq!(hall.diffuse_share(0), 0.0);
        assert_eq!(hall.diffuse_share(3), 1.0);
        assert_eq!(
            o.names(),
            ["wall-cutoff", "reverb-decay", "reflection-diffusion"]
        );
    }

    #[test]
    fn taps_pair_up_in_order_sorted_by_delay() {
        // New delays with the preset's four levels: three taps.
        let o = RoomOverrides {
            delays_ms: vec![30.0, 10.0, 20.0],
            ..RoomOverrides::default()
        };
        let p = Room::Hall.params(&o);
        assert_eq!(delays(&p), [10.0, 20.0, 30.0]);
        assert_eq!(p.taps[0].gain_db, -12.0); // 10 ms took the second level
        assert!((p.diffusion - 1.0 / 3.0).abs() < 1e-12);
        // Six levels and six delays: six taps; within limits.
        let o = RoomOverrides {
            delays_ms: vec![1.0, 2.0, 3.0, 4.0, 5.0, 900.0],
            levels_db: vec![-6.0; 6],
            ..RoomOverrides::default()
        };
        assert_eq!(
            delays(&Room::Small.params(&o)),
            [1.0, 2.0, 3.0, 4.0, 5.0, MAX_TAP_DELAY_MS]
        );
    }

    #[test]
    fn the_treble_never_outlasts_the_bass_and_outdoors_stays_open() {
        let o = RoomOverrides {
            decay_s: Some(0.5),
            treble_decay_s: Some(3.0),
            size_ms: Some(0.0),
            delays_ms: vec![10.0],
            levels_db: vec![-6.0],
            cutoff_hz: Some(2000.0),
            ..RoomOverrides::default()
        };
        let reverb = Room::Cave.params(&o).reverb.unwrap();
        assert_eq!((reverb.treble_decay_s, reverb.size_ms), (0.5, MIN_SIZE_MS));
        let outdoor = Room::Outdoor.params(&o);
        assert!(!outdoor.reflects());
        assert_eq!(outdoor.cutoff_hz, f64::INFINITY);
    }
}
