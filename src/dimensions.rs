// Copyright 2026 Simon Annetts/Jak Daniels
// SPDX-License-Identifier: MPL-2.0

//! Room dimensions (#8): a box room's size, what its walls, floor and ceiling are made of, and what is in it give
//! the values a preset would otherwise (its early reflections, the walls' cutoff and the late reverb), so a room can
//! be described instead of tuned. Computed when the room changes, never per sample.
//!
//! - Decay: Eyring's formula per band, T = 0.161 V / (−S ln(1 − ā) + A + 4mV), with ā the surfaces' mean absorption,
//!   A the absorption of the room's contents (furniture and people, by floor area) and m the air's (treble only,
//!   from `air-absorption`).
//! - Size: the mean free path 4V/S, as a time.
//! - Reflections: the first-order image sources of the six surfaces for a speaker and listener together in the
//!   middle of the room at ear height; opposite walls equally far away arrive together (one tap, twice the energy).
//!   Level by path length against the reference distance, less what the surface absorbs.
//! - Walls' cutoff: the one-pole low-pass that gives the surfaces' treble loss against their bass loss at 4 kHz.
//! - Reverb level: the diffuse field against the direct sound at the reference distance, 16πr²(1 − ā) / Sā,
//!   compressed (half as many dB) and limited, as it spans 30 dB from a tiled closet to a hall where the presets,
//!   tuned by ear, keep within a few dB of −15.
//!
//! The levels are set so that typical rooms come out near the presets, which were tuned by ear quieter than the
//! physics (a crowded 30 × 20 × 12 m hall: −15 dB, as the hall preset). The materials and contents are to be tuned
//! by ear too.

use gst::glib;

use crate::rooms::{MAX_TAP_DELAY_MS, Reverb, Tap};
use crate::spatial::{NO_FILTER_HZ, SPEED_OF_SOUND};

/// Speakers' and listeners' ears above the floor (m), at most half the room's height.
const EAR_HEIGHT: f64 = 1.6;
/// Where the surfaces' treble absorption is given, for the walls' cutoff (Hz).
const TREBLE_HZ: f64 = 4000.0;
/// Physics against the presets tuned by ear (dB): reflections come out this much quieter.
const REFLECTION_TRIM_DB: f64 = -6.0;
/// The reverb's level: the physical one's dB times the slope, plus the offset, within the limits.
const REVERB_SLOPE: f64 = 0.5;
const REVERB_OFFSET_DB: f64 = -11.5;
const REVERB_LIMITS_DB: (f64, f64) = (-20.0, -10.0);
/// The most a surface's mean absorption is taken as (all of it would be no room at all).
const MAX_ABSORPTION: f64 = 0.99;

/// What a surface is made of: its absorption in the bass and in the treble (about 250 Hz and 4 kHz).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, glib::Enum)]
#[repr(u32)]
#[enum_type(name = "GstAcousticMultiMixerMaterial")]
pub enum Material {
    #[enum_value(name = "Open: no surface (absorbs everything)", nick = "open")]
    Open,
    #[enum_value(name = "Concrete: bare or painted", nick = "concrete")]
    Concrete,
    #[enum_value(name = "Stone: smooth stone or marble", nick = "stone")]
    Stone,
    #[enum_value(name = "Brick: bare brick", nick = "brick")]
    Brick,
    #[enum_value(name = "Tile: glazed tiles", nick = "tile")]
    Tile,
    #[default]
    #[enum_value(name = "Plaster: on masonry", nick = "plaster")]
    Plaster,
    #[enum_value(
        name = "Plasterboard: on studs (absorbs some bass)",
        nick = "plasterboard"
    )]
    Plasterboard,
    #[enum_value(name = "Wood: floorboards or panelling", nick = "wood")]
    Wood,
    #[enum_value(name = "Glass: large windows", nick = "glass")]
    Glass,
    #[enum_value(name = "Metal: sheet metal", nick = "metal")]
    Metal,
    #[enum_value(name = "Carpet: on a solid floor", nick = "carpet")]
    Carpet,
    #[enum_value(name = "Curtain: heavy, draped", nick = "curtain")]
    Curtain,
    #[enum_value(
        name = "Acoustic tile: a suspended absorbent ceiling",
        nick = "acoustic-tile"
    )]
    AcousticTile,
    #[enum_value(name = "Water: a pool or lake", nick = "water")]
    Water,
    #[enum_value(name = "Earth: packed soil or a rough cave floor", nick = "earth")]
    Earth,
    #[enum_value(name = "Grass: a lawn", nick = "grass")]
    Grass,
}

impl Material {
    /// Absorption in the bass and in the treble.
    pub fn absorption(self) -> [f64; 2] {
        match self {
            Material::Open => [1.0, 1.0],
            Material::Concrete => [0.01, 0.02],
            Material::Stone => [0.02, 0.04],
            Material::Brick => [0.03, 0.07],
            Material::Tile => [0.01, 0.02],
            Material::Plaster => [0.02, 0.05],
            Material::Plasterboard => [0.10, 0.05],
            Material::Wood => [0.15, 0.07],
            Material::Glass => [0.25, 0.03],
            Material::Metal => [0.05, 0.05],
            Material::Carpet => [0.05, 0.60],
            Material::Curtain => [0.10, 0.60],
            Material::AcousticTile => [0.40, 0.80],
            Material::Water => [0.01, 0.02],
            Material::Earth => [0.15, 0.40],
            Material::Grass => [0.10, 0.60],
        }
    }
}

/// What is in the room, absorbing sound: furniture and people.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, glib::Enum)]
#[repr(u32)]
#[enum_type(name = "GstAcousticMultiMixerRoomContents")]
pub enum Contents {
    #[enum_value(name = "Empty: bare surfaces only", nick = "empty")]
    Empty,
    #[enum_value(name = "Sparse: a few chairs and people", nick = "sparse")]
    Sparse,
    #[default]
    #[enum_value(
        name = "Furnished: a lived-in room, an office, a café",
        nick = "furnished"
    )]
    Furnished,
    #[enum_value(
        name = "Crowded: full of people, or soft furnishings",
        nick = "crowded"
    )]
    Crowded,
}

impl Contents {
    /// Absorption (m² of open window) per m² of floor, in the bass and in the treble.
    pub fn absorption(self) -> [f64; 2] {
        match self {
            Contents::Empty => [0.0, 0.0],
            Contents::Sparse => [0.2, 0.3],
            Contents::Furnished => [0.6, 0.8],
            Contents::Crowded => [1.0, 1.2],
        }
    }
}

/// The surfaces, in the order of `RoomDimensions`' arrays: their material and absorption properties.
pub const MATERIAL_PROPERTIES: [&str; 3] = ["wall-material", "floor-material", "ceiling-material"];
pub const ABSORPTION_PROPERTIES: [&str; 3] =
    ["wall-absorption", "floor-absorption", "ceiling-absorption"];
/// The size properties, in the order of `RoomDimensions::size`.
pub const SIZE_PROPERTIES: [&str; 3] = ["room-width", "room-depth", "room-height"];
/// The room-override properties whose value dimensions give.
pub const DERIVED_PROPERTIES: [&str; 6] = [
    "reflection-delays",
    "reflection-levels",
    "wall-cutoff",
    "reverb-decay",
    "reverb-treble-decay",
    "reverb-size",
];

/// A room's dimensions as its properties set them.
#[derive(Debug, Clone, PartialEq)]
pub struct RoomDimensions {
    /// Width (east–west), depth (north–south) and height (m); any of them 0: not given.
    pub size: [f64; 3],
    /// Walls, floor and ceiling.
    pub materials: [Material; 3],
    /// Overrides of the materials' absorption: empty = the material's, one value = both bands, two = bass, treble.
    pub absorption: [Vec<f64>; 3],
    pub contents: Contents,
}

impl Default for RoomDimensions {
    fn default() -> Self {
        RoomDimensions {
            size: [0.0; 3],
            materials: [Material::Plaster, Material::Wood, Material::Plaster],
            absorption: Default::default(),
            contents: Contents::default(),
        }
    }
}

/// What a room's dimensions give.
#[derive(Debug, Clone, PartialEq)]
pub struct Derived {
    pub taps: Vec<Tap>,
    pub cutoff_hz: f64,
    pub reverb: Reverb,
}

impl RoomDimensions {
    pub fn given(&self) -> bool {
        self.size.iter().all(|&s| s > 0.0)
    }

    /// A surface's absorption in effect, bass and treble.
    pub fn absorption(&self, surface: usize) -> [f64; 2] {
        let a = match self.absorption[surface][..] {
            [] => self.materials[surface].absorption(),
            [both] => [both, both],
            [bass, treble, ..] => [bass, treble],
        };
        a.map(|v| v.clamp(0.0, 1.0))
    }

    /// The room's values, for voices heard at `reference` (m) through air absorbing `air_db_per_m` at 8 kHz; None
    /// without a size.
    pub fn derive(&self, reference: f64, air_db_per_m: f64) -> Option<Derived> {
        if !self.given() {
            return None;
        }
        let [w, d, h] = self.size;
        let floor = w * d;
        let areas = [2.0 * h * (w + d), floor, floor];
        let total: f64 = areas.iter().sum();
        let volume = w * d * h;
        let absorption = [0, 1, 2].map(|s| self.absorption(s));
        let mean = |band: usize| {
            (areas
                .iter()
                .zip(&absorption)
                .map(|(area, a)| area * a[band])
                .sum::<f64>()
                / total)
                .clamp(1e-4, MAX_ABSORPTION)
        };
        let (bass, treble) = (mean(0), mean(1));
        let contents = self.contents.absorption().map(|a| a * floor);
        // Intensity lost per metre to the air (treble), from dB.
        let air = air_db_per_m / (10.0 * std::f64::consts::LOG10_E);
        let decay = |a: f64, extra: f64, air: f64| {
            0.161 * volume / (-total * (1.0 - a).ln() + extra + 4.0 * air * volume)
        };
        let decay_s = decay(bass, contents[0], 0.0);
        let treble_decay_s = decay(treble, contents[1], air).min(decay_s);
        let free_path = 4.0 * volume / total;

        // The reverb's level: the diffuse field's energy against the direct sound's at the reference distance, the
        // contents counted in the absorption.
        let r = reference.max(0.1);
        let absorbed = total * bass + contents[0];
        let diffuse =
            16.0 * std::f64::consts::PI * r * r * (1.0 - absorbed / total).max(0.01) / absorbed;
        let level_db = (REVERB_SLOPE * 10.0 * diffuse.log10() + REVERB_OFFSET_DB)
            .clamp(REVERB_LIMITS_DB.0, REVERB_LIMITS_DB.1);

        // First-order images for a speaker and listener together in the middle at ear height: the side walls (twice
        // each), the floor and the ceiling.
        let ear = EAR_HEIGHT.min(h / 2.0);
        let mut taps: Vec<(f64, f64)> = vec![]; // path (m), energy
        for (path, surface, count) in [
            (w, 0, 2.0),
            (d, 0, 2.0),
            (2.0 * ear, 1, 1.0),
            (2.0 * (h - ear), 2, 1.0),
        ] {
            let energy = count * (r / path).powi(2) * (1.0 - absorption[surface][0]);
            match taps.iter_mut().find(|(p, _)| (p - path).abs() < 1e-6) {
                Some(tap) => tap.1 += energy,
                None => taps.push((path, energy)),
            }
        }
        let taps = taps
            .into_iter()
            .filter(|&(_, energy)| energy > 0.0)
            .map(|(path, energy)| Tap {
                delay_ms: (path / SPEED_OF_SOUND * 1000.0).min(MAX_TAP_DELAY_MS),
                gain_db: (10.0 * energy.log10() + REFLECTION_TRIM_DB).min(0.0),
            })
            .collect::<Vec<_>>();

        // The walls' cutoff: the surfaces' treble kept against their bass (amplitude), given by a one-pole
        // low-pass at 4 kHz.
        let kept = |band: usize| {
            areas
                .iter()
                .zip(&absorption)
                .map(|(area, a)| area * (1.0 - a[band]))
                .sum::<f64>()
        };
        let ratio = (kept(1) / kept(0).max(1e-9)).sqrt();
        let cutoff_hz = if ratio >= 0.999 || kept(0) <= 0.0 {
            f64::INFINITY
        } else {
            TREBLE_HZ / (1.0 / (ratio * ratio).max(1e-6) - 1.0).sqrt()
        };
        // Above the band, no filter at all.
        let cutoff_hz = if cutoff_hz >= NO_FILTER_HZ {
            f64::INFINITY
        } else {
            cutoff_hz
        };

        Some(Derived {
            taps,
            cutoff_hz,
            reverb: Reverb {
                decay_s,
                treble_decay_s,
                level_db,
                size_ms: free_path / SPEED_OF_SOUND * 1000.0,
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rooms::{Room, RoomOverrides};

    /// A 10 m cube absorbing half of what reaches every surface, empty.
    fn cube() -> RoomDimensions {
        RoomDimensions {
            size: [10.0; 3],
            absorption: [vec![0.5], vec![0.5], vec![0.5]],
            contents: Contents::Empty,
            ..RoomDimensions::default()
        }
    }

    #[test]
    fn a_room_needs_all_three_sizes() {
        let mut r = cube();
        r.size[2] = 0.0;
        assert!(r.derive(2.0, 0.0).is_none());
        assert!(cube().derive(2.0, 0.0).is_some());
    }

    #[test]
    fn decay_follows_eyring() {
        // V = 1000, S = 600: 0.161 × 1000 / (600 × ln 2) = 0.387 s, the same in the treble without air.
        let d = cube().derive(2.0, 0.0).unwrap();
        assert!(
            (d.reverb.decay_s - 0.387).abs() < 0.001,
            "{}",
            d.reverb.decay_s
        );
        assert_eq!(d.reverb.treble_decay_s, d.reverb.decay_s);
        // The mean free path 4V/S = 6.67 m: 19.4 ms.
        assert!((d.reverb.size_ms - 19.44).abs() < 0.01);

        // The air takes the treble only; contents, absorbent surfaces and size change both.
        let aired = cube().derive(2.0, 0.1).unwrap();
        assert_eq!(aired.reverb.decay_s, d.reverb.decay_s);
        assert!(aired.reverb.treble_decay_s < d.reverb.treble_decay_s);
        let furnished = RoomDimensions {
            contents: Contents::Furnished,
            ..cube()
        };
        assert!(furnished.derive(2.0, 0.0).unwrap().reverb.decay_s < d.reverb.decay_s);
        let bigger = RoomDimensions {
            size: [20.0; 3],
            ..cube()
        };
        assert!(bigger.derive(2.0, 0.0).unwrap().reverb.decay_s > d.reverb.decay_s);
    }

    #[test]
    fn reflections_come_from_the_six_surfaces() {
        let d = cube().derive(2.0, 0.0).unwrap();
        // Four walls 5 m away arrive together (10 m there and back), then the floor below the ears (3.2 m) and the
        // ceiling (16.8 m).
        let mut taps = d.taps.clone();
        taps.sort_by(|a, b| a.delay_ms.total_cmp(&b.delay_ms));
        let delays: Vec<f64> = taps
            .iter()
            .map(|t| (t.delay_ms * 10.0).round() / 10.0)
            .collect();
        assert_eq!(delays, [9.3, 29.2, 49.0]);
        // The floor's: (2 / 3.2)² × half, less the trim: −4.1 − 3.0 − 6 dB.
        assert!(
            (taps[0].gain_db - (-13.09)).abs() < 0.01,
            "{}",
            taps[0].gain_db
        );
        // The walls': four of them, (2 / 10)² × half each.
        let walls = 10.0 * (4.0 * 0.04 * 0.5f64).log10() + REFLECTION_TRIM_DB;
        assert!((taps[1].gain_db - walls).abs() < 1e-9);
    }

    #[test]
    fn soft_surfaces_dull_the_reflections() {
        let hard = RoomDimensions {
            size: [5.0, 5.0, 3.0],
            materials: [Material::Concrete; 3],
            ..RoomDimensions::default()
        };
        assert_eq!(hard.derive(2.0, 0.0).unwrap().cutoff_hz, f64::INFINITY);
        let carpeted = RoomDimensions {
            materials: [Material::Concrete, Material::Carpet, Material::Concrete],
            ..hard.clone()
        };
        let cutoff = carpeted.derive(2.0, 0.0).unwrap().cutoff_hz;
        assert!(cutoff > 1000.0 && cutoff < 20000.0, "{cutoff}");
        // A material is overridden by numbers, one for both bands or two.
        let mut r = carpeted.clone();
        r.absorption[1] = vec![0.2];
        assert_eq!(r.absorption(1), [0.2, 0.2]);
        r.absorption[1] = vec![0.1, 0.7];
        assert_eq!(r.absorption(1), [0.1, 0.7]);
        assert_eq!(r.absorption(0), Material::Concrete.absorption());
    }

    #[test]
    fn the_reverb_level_stays_near_the_presets() {
        let level = |size: [f64; 3], contents| {
            RoomDimensions {
                size,
                contents,
                ..RoomDimensions::default()
            }
            .derive(2.0, 0.1)
            .unwrap()
            .reverb
            .level_db
        };
        // A crowded hall as the hall preset; a closet as wet as allowed; a small room in between.
        assert!((level([30.0, 20.0, 12.0], Contents::Crowded) - -15.0).abs() < 0.5);
        assert_eq!(level([1.5, 2.0, 2.4], Contents::Empty), REVERB_LIMITS_DB.1);
        let small = level([4.0, 5.0, 2.7], Contents::Furnished);
        assert!((REVERB_LIMITS_DB.0..=REVERB_LIMITS_DB.1).contains(&small));
    }

    #[test]
    fn dimensions_take_the_presets_place_and_overrides_theirs() {
        let derived = cube().derive(2.0, 0.0);
        // Outdoors with dimensions is a room.
        let outdoor = Room::Outdoor.params(&RoomOverrides::default(), derived.as_ref());
        assert!(outdoor.reflects());
        assert_eq!(outdoor.taps.len(), 3);
        let hall = Room::Hall.params(&RoomOverrides::default(), derived.as_ref());
        assert_eq!(hall.reverb, outdoor.reverb);
        // An override still wins.
        let o = RoomOverrides {
            decay_s: Some(2.0),
            ..RoomOverrides::default()
        };
        let p = Room::Hall.params(&o, derived.as_ref());
        assert_eq!(p.reverb.unwrap().decay_s, 2.0);
        assert_eq!(p.reverb.unwrap().size_ms, hall.reverb.unwrap().size_ms);
    }
}
