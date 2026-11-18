// Copyright 2026 Simon Annetts/Jak Daniels
// SPDX-License-Identifier: MPL-2.0

//! How one voice sounds to one listener: a level from the distance, and a stereo pan from the direction. The
//! formulas are SpatialAudioServer's (`Media/Spatial/SpatialPolicy.cs`), so its rooms sound the same with this
//! element as with its `matrix` engine.
//!
//! Coordinates follow SL/OpenSim: x east, y north, z up. Angles are in degrees. With yaw, pitch and roll all 0 a
//! participant faces +x, so their left is +y. Yaw turns to the left (towards +y), pitch looks up, roll lowers the
//! right ear. Together they are the rotation Rz(yaw) · Ry(−pitch) · Rx(roll): for a quaternion q = (x, y, z, w),
//! yaw = atan2(2(wz + xy), 1 − 2(y² + z²)), pitch = −asin(2(wy − zx)), roll = atan2(2(wx + yz), 1 − 2(x² + y²)).

use gst::glib;

/// How volume falls with distance beyond the reference distance (the OpenAL / Web Audio models).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, glib::Enum)]
#[repr(u32)]
#[enum_type(name = "GstAcousticMultiMixerDistanceModel")]
pub enum DistanceModel {
    /// reference / (reference + rolloff × (d − reference)); rolloff 1 is reference / d, −6 dB per doubling.
    #[enum_value(
        name = "Inverse: reference / (reference + rolloff × (distance − reference))",
        nick = "inverse"
    )]
    Inverse,

    /// 1 − rolloff × (d − reference) / (max range − reference), down to silence (SL's Vivox default).
    #[enum_value(
        name = "Linear: 1 − rolloff × (distance − reference) / (max-range − reference)",
        nick = "linear"
    )]
    Linear,

    /// (d / reference)^−rolloff.
    #[default]
    #[enum_value(
        name = "Exponential: (distance / reference)^−rolloff",
        nick = "exponential"
    )]
    Exponential,
}

/// The distance curve. The defaults are SpatialAudioServer's `outdoor` profile: −6 dB per doubling from 2 m.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DistanceParams {
    pub model: DistanceModel,
    /// Sources at or within this distance are heard at full volume.
    pub reference: f64,
    pub rolloff: f64,
    /// Beyond this distance a source is silent and left out of the mix.
    pub max_range: f64,
}

impl Default for DistanceParams {
    fn default() -> Self {
        DistanceParams {
            model: DistanceModel::Exponential,
            reference: 2.0,
            rolloff: 1.0,
            max_range: 60.0,
        }
    }
}

impl DistanceParams {
    /// The distance model's gain, 0 to 1.
    pub fn attenuation(&self, distance: f64) -> f64 {
        let (reference, rolloff) = (self.reference, self.rolloff);
        if distance <= reference {
            return 1.0;
        }
        match self.model {
            DistanceModel::Linear => {
                if self.max_range <= reference {
                    0.0
                } else {
                    (1.0 - rolloff * (distance - reference) / (self.max_range - reference))
                        .clamp(0.0, 1.0)
                }
            }
            DistanceModel::Exponential => (distance / reference).powf(-rolloff),
            DistanceModel::Inverse => reference / (reference + rolloff * (distance - reference)),
        }
    }
}

/// Which way someone faces, in degrees (see the module documentation).
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct Orientation {
    pub yaw: f64,
    pub pitch: f64,
    pub roll: f64,
}

impl Orientation {
    /// A world-space vector in this orientation's frame: (forward, left, up).
    pub fn to_local(self, v: [f64; 3]) -> [f64; 3] {
        let (sy, cy) = self.yaw.to_radians().sin_cos();
        let (sp, cp) = self.pitch.to_radians().sin_cos();
        let (sr, cr) = self.roll.to_radians().sin_cos();
        // The inverse of Rz(yaw) · Ry(−pitch) · Rx(roll): Rx(−roll) · Ry(pitch) · Rz(−yaw).
        let x1 = cy * v[0] + sy * v[1];
        let y1 = -sy * v[0] + cy * v[1];
        let z1 = v[2];
        let x2 = cp * x1 + sp * z1;
        let z2 = -sp * x1 + cp * z1;
        let y3 = cr * y1 + sr * z2;
        let z3 = -sr * y1 + cr * z2;
        [x2, y3, z3]
    }
}

/// How one source sounds to one listener: a level (0 = silent, out of the mix), a pan (−1 left … 1 right: the
/// sine of the source's angle from straight ahead), how far behind it is (0 at the side or in front … 1 straight
/// behind: the cosine of its angle from straight back), and the distance (0 without positions).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rendering {
    pub gain: f64,
    pub pan: f64,
    pub behind: f64,
    pub distance: f64,
}

impl Rendering {
    pub const SILENT: Rendering = Rendering {
        gain: 0.0,
        pan: 0.0,
        behind: 0.0,
        distance: 0.0,
    };

    /// Full volume, centred: a participant without a position.
    pub const CENTRE: Rendering = Rendering {
        gain: 1.0,
        pan: 0.0,
        behind: 0.0,
        distance: 0.0,
    };

    /// Left and right gains. Constant power with the centre at unity on both channels (so a centred source is
    /// exactly as loud as with no positions at all), capped at unity so panning never boosts: centre 1/1, hard
    /// right 0/1.
    pub fn stereo(&self) -> (f64, f64) {
        let angle = (self.pan.clamp(-1.0, 1.0) + 1.0) * std::f64::consts::FRAC_PI_4;
        let root2 = std::f64::consts::SQRT_2;
        (
            self.gain * (root2 * angle.cos()).min(1.0),
            self.gain * (root2 * angle.sin()).min(1.0),
        )
    }
}

/// Renders a source for a listener. Distance: full volume up to the reference distance, then the distance model,
/// silent beyond the maximum range. Direction: the pan is the sine of the source's horizontal angle from where the
/// listener faces, so ahead or behind is centred and directly left or right is hard left or right (stereo cannot
/// tell front from back). Elevation only affects distance. Either side without a position hears or is heard
/// centred at full volume.
pub fn render(
    source: Option<[f64; 3]>,
    listener: Option<([f64; 3], Orientation)>,
    params: &DistanceParams,
) -> Rendering {
    let (Some(source), Some((listener, orientation))) = (source, listener) else {
        return Rendering::CENTRE;
    };
    let d = [
        source[0] - listener[0],
        source[1] - listener[1],
        source[2] - listener[2],
    ];
    let distance = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
    if distance > params.max_range {
        return Rendering::SILENT;
    }
    let gain = params.attenuation(distance);
    if gain <= 0.0 {
        return Rendering::SILENT;
    }
    let [forward, left, _] = orientation.to_local(d);
    let horizontal = (forward * forward + left * left).sqrt();
    let behind = if horizontal < 1e-6 {
        0.0
    } else {
        (-forward / horizontal).max(0.0)
    };
    let pan = if horizontal < 1e-6 {
        0.0
    } else {
        -left / horizontal
    };
    Rendering {
        gain,
        pan,
        behind,
        distance,
    }
}

/// How direction is heard.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, glib::Enum)]
#[repr(u32)]
#[enum_type(name = "GstAcousticMultiMixerDirectionModel")]
pub enum DirectionModel {
    /// Constant-power stereo pan: a source to one side is silent in the other ear (SpatialAudioServer's `matrix`
    /// engine).
    #[enum_value(
        name = "Pan: constant-power stereo pan (silent in the far ear at the side)",
        nick = "pan"
    )]
    Pan,

    /// A head between the ears: the far ear hears the source later (ITD), quieter (ILD) and duller (head shadow).
    #[default]
    #[enum_value(
        name = "Head: the far ear hears later, quieter and duller (ITD, ILD, head shadow)",
        nick = "head"
    )]
    Head,
}

/// The element's acoustic settings beyond the distance curve.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Acoustics {
    pub direction: DirectionModel,
    /// High-frequency loss with distance, in dB per metre at 8 kHz (air: about 0.1); 0 = none.
    pub air_absorption: f64,
    /// Head model: the low-pass cut-off (Hz) for a source straight behind, as the outer ears shade sounds from
    /// behind; none at the side and in front. 0 = none.
    pub rear_shadow: f64,
    /// The space, for its early reflections.
    pub room: crate::rooms::Room,
}

impl Default for Acoustics {
    fn default() -> Self {
        Acoustics {
            direction: DirectionModel::Head,
            air_absorption: DEFAULT_AIR_ABSORPTION,
            rear_shadow: DEFAULT_REAR_SHADOW,
            room: crate::rooms::Room::Outdoor,
        }
    }
}

pub const DEFAULT_AIR_ABSORPTION: f64 = 0.1;
/// About 3 dB down at 4 kHz and 7 dB at 8 kHz straight behind (to be tuned by ear).
pub const DEFAULT_REAR_SHADOW: f64 = 4000.0;

/// Head model constants (to be tuned by ear). Woodworth's ITD for a spherical head: (r / c)(θ + sin θ), up to
/// 0.66 ms at the side.
const HEAD_RADIUS: f64 = 0.0875; // m
const SPEED_OF_SOUND: f64 = 343.0; // m/s
/// Broadband level difference at the side (design.md: −8 dB; less here, since the head shadow adds to it).
const ILD_SIDE_DB: f64 = 6.0;
/// Far-ear low-pass at the side (Hz), easing towards none straight ahead.
const SHADOW_SIDE_HZ: f64 = 1800.0;
/// Above this a low-pass is not worth running.
pub const NO_FILTER_HZ: f64 = 18000.0;

/// The longest ITD the head model gives, in seconds.
pub const MAX_ITD: f64 = HEAD_RADIUS / SPEED_OF_SOUND * (std::f64::consts::FRAC_PI_2 + 1.0);

/// What one ear gets of one source: a gain, a delay in seconds, and a one-pole low-pass cut-off in Hz (infinite =
/// none).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ear {
    pub gain: f64,
    pub delay: f64,
    pub cutoff: f64,
}

impl Ear {
    pub const SILENT: Ear = Ear {
        gain: 0.0,
        delay: 0.0,
        cutoff: f64::INFINITY,
    };
}

/// The low-pass cut-off for air absorption over `distance` metres: a one-pole filter losing
/// `db_per_metre × distance` dB at 8 kHz.
fn air_cutoff(distance: f64, db_per_metre: f64) -> f64 {
    let loss = db_per_metre * distance;
    if loss <= 0.0 {
        return f64::INFINITY;
    }
    8000.0 / (10f64.powf(loss / 10.0) - 1.0).sqrt()
}

/// Left and right ears for a rendering.
pub fn ears(r: &Rendering, acoustics: &Acoustics) -> [Ear; 2] {
    if r.gain <= 0.0 {
        return [Ear::SILENT; 2];
    }
    let air = air_cutoff(r.distance, acoustics.air_absorption);
    match acoustics.direction {
        DirectionModel::Pan => {
            let (l, r) = r.stereo();
            [l, r].map(|gain| Ear {
                gain,
                delay: 0.0,
                cutoff: air,
            })
        }
        DirectionModel::Head => {
            // s = sin of the lateral angle (0 ahead or behind, 1 at the side).
            let s = r.pan.abs().min(1.0);
            // Both ears: air absorption, and the rear shadow easing in from the side to straight behind.
            let behind = r.behind.min(1.0);
            let both = if acoustics.rear_shadow > 0.0 && behind > 0.0 {
                (20000.0 * (acoustics.rear_shadow.min(20000.0) / 20000.0).powf(behind)).min(air)
            } else {
                air
            };
            let near = Ear {
                gain: r.gain,
                delay: 0.0,
                cutoff: both,
            };
            let lateral = s.asin();
            let far = Ear {
                gain: r.gain * 10f64.powf(-ILD_SIDE_DB * s / 20.0),
                delay: HEAD_RADIUS / SPEED_OF_SOUND * (lateral + s),
                cutoff: if s > 0.0 {
                    (20000.0 * (SHADOW_SIDE_HZ / 20000.0).powf(s)).min(both)
                } else {
                    both
                },
            };
            if r.pan > 0.0 {
                [far, near]
            } else {
                [near, far]
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db(gain: f64) -> f64 {
        20.0 * gain.log10()
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn outdoor_falls_6_db_per_doubling_from_2_m() {
        let p = DistanceParams::default();
        assert_eq!(p.attenuation(0.5), 1.0);
        assert_eq!(p.attenuation(2.0), 1.0);
        assert!((db(p.attenuation(4.0)) + 6.02).abs() < 0.01);
        assert!((db(p.attenuation(5.0)) + 7.96).abs() < 0.01);
        assert!((db(p.attenuation(20.0)) + 20.0).abs() < 0.01);
        assert!((db(p.attenuation(50.0)) + 27.96).abs() < 0.01);
    }

    #[test]
    fn meeting_is_linear_from_10_to_35_m() {
        let p = DistanceParams {
            model: DistanceModel::Linear,
            reference: 10.0,
            rolloff: 2.0,
            max_range: 60.0,
        };
        assert_eq!(p.attenuation(10.0), 1.0);
        assert!(close(p.attenuation(22.5), 0.5));
        assert_eq!(p.attenuation(35.0), 0.0);
        assert_eq!(p.attenuation(50.0), 0.0);
    }

    #[test]
    fn inverse_with_rolloff() {
        let p = DistanceParams {
            model: DistanceModel::Inverse,
            reference: 1.0,
            rolloff: 2.0,
            max_range: 100.0,
        };
        assert!(close(p.attenuation(3.0), 1.0 / 5.0));
    }

    #[test]
    fn stereo_is_unity_in_the_centre_and_never_boosts() {
        let (l, r) = Rendering::CENTRE.stereo();
        assert!(close(l, 1.0) && close(r, 1.0));
        let (l, r) = Rendering {
            gain: 1.0,
            pan: 1.0,
            behind: 0.0,
            distance: 0.0,
        }
        .stereo();
        assert!(close(l, 0.0) && close(r, 1.0));
        let (l, r) = Rendering {
            gain: 0.5,
            pan: -1.0,
            behind: 0.0,
            distance: 0.0,
        }
        .stereo();
        assert!(close(l, 0.5) && close(r, 0.0));
        let (l, r) = Rendering {
            gain: 1.0,
            pan: 0.5,
            behind: 0.0,
            distance: 0.0,
        }
        .stereo();
        assert!(l < 1.0 && close(r, 1.0));
    }

    fn at(x: f64, y: f64, z: f64) -> Option<[f64; 3]> {
        Some([x, y, z])
    }

    fn facing(yaw: f64, pitch: f64, roll: f64) -> Option<([f64; 3], Orientation)> {
        Some(([0.0; 3], Orientation { yaw, pitch, roll }))
    }

    #[test]
    fn direction() {
        let p = DistanceParams::default();
        // Facing +x: +y is to the left, −y to the right; ahead and behind are centred.
        assert!(close(
            render(at(0.0, 1.0, 0.0), facing(0.0, 0.0, 0.0), &p).pan,
            -1.0
        ));
        assert!(close(
            render(at(0.0, -1.0, 0.0), facing(0.0, 0.0, 0.0), &p).pan,
            1.0
        ));
        assert!(close(
            render(at(1.0, 0.0, 0.0), facing(0.0, 0.0, 0.0), &p).pan,
            0.0
        ));
        assert!(close(
            render(at(-1.0, 0.0, 0.0), facing(0.0, 0.0, 0.0), &p).pan,
            0.0
        ));
        // Turned left 90°: +y is ahead, +x is to the right.
        assert!(close(
            render(at(0.0, 1.0, 0.0), facing(90.0, 0.0, 0.0), &p).pan,
            0.0
        ));
        assert!(close(
            render(at(1.0, 0.0, 0.0), facing(90.0, 0.0, 0.0), &p).pan,
            1.0
        ));
        // 45° to the front left.
        let r = render(at(1.0, 1.0, 0.0), facing(0.0, 0.0, 0.0), &p);
        assert!(close(r.pan, -std::f64::consts::FRAC_1_SQRT_2));
        // Upside down (roll 180°): left and right swap.
        assert!(close(
            render(at(0.0, 1.0, 0.0), facing(0.0, 0.0, 180.0), &p).pan,
            1.0
        ));
        // Looking straight up: something ahead is now below, centred.
        assert!(close(
            render(at(1.0, 0.0, 0.0), facing(0.0, 90.0, 0.0), &p).pan,
            0.0
        ));
    }

    #[test]
    fn distance_and_range() {
        let p = DistanceParams::default();
        let r = render(at(20.0, 0.0, 0.0), facing(0.0, 0.0, 0.0), &p);
        assert!(close(r.gain, 0.1));
        // Elevation counts towards distance.
        let r = render(at(0.0, 0.0, 4.0), facing(0.0, 0.0, 0.0), &p);
        assert!(close(r.gain, 0.5) && close(r.pan, 0.0));
        assert_eq!(
            render(at(61.0, 0.0, 0.0), facing(0.0, 0.0, 0.0), &p),
            Rendering::SILENT
        );
        // Without a position on either side: centred at full volume.
        assert_eq!(render(None, facing(0.0, 0.0, 0.0), &p), Rendering::CENTRE);
        assert_eq!(render(at(30.0, 0.0, 0.0), None, &p), Rendering::CENTRE);
    }

    /// SpatialAudioServer's quaternion rotation (DistancePolicy.InverseRotate), for comparison.
    fn inverse_rotate(q: [f64; 4], v: [f64; 3]) -> [f64; 3] {
        let norm = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
        let (qx, qy, qz, qw) = (-q[0] / norm, -q[1] / norm, -q[2] / norm, q[3] / norm);
        let (x, y, z) = (v[0], v[1], v[2]);
        let (cx, cy, cz) = (qy * z - qz * y, qz * x - qx * z, qx * y - qy * x);
        let (ccx, ccy, ccz) = (qy * cz - qz * cy, qz * cx - qx * cz, qx * cy - qy * cx);
        [
            x + 2.0 * (qw * cx + ccx),
            y + 2.0 * (qw * cy + ccy),
            z + 2.0 * (qw * cz + ccz),
        ]
    }

    /// The module documentation's quaternion → yaw/pitch/roll conversion.
    fn from_quaternion(q: [f64; 4]) -> Orientation {
        let norm = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
        let (x, y, z, w) = (q[0] / norm, q[1] / norm, q[2] / norm, q[3] / norm);
        Orientation {
            yaw: (2.0 * (w * z + x * y))
                .atan2(1.0 - 2.0 * (y * y + z * z))
                .to_degrees(),
            pitch: -(2.0 * (w * y - z * x)).clamp(-1.0, 1.0).asin().to_degrees(),
            roll: (2.0 * (w * x + y * z))
                .atan2(1.0 - 2.0 * (x * x + y * y))
                .to_degrees(),
        }
    }

    #[test]
    fn euler_angles_match_spatialaudioserver_quaternions() {
        // A fixed pseudo-random set of orientations and directions.
        let mut seed = 7u64;
        let mut next = || {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((seed >> 11) as f64 / (1u64 << 53) as f64) * 2.0 - 1.0
        };
        for _ in 0..1000 {
            let q = [next(), next(), next(), next()];
            let v = [next() * 10.0, next() * 10.0, next() * 10.0];
            let expected = inverse_rotate(q, v);
            let got = from_quaternion(q).to_local(v);
            for i in 0..3 {
                assert!(
                    (expected[i] - got[i]).abs() < 1e-9,
                    "q {q:?} v {v:?}: {expected:?} vs {got:?}"
                );
            }
        }
    }

    #[test]
    fn head_model_ears() {
        let head = Acoustics {
            direction: DirectionModel::Head,
            air_absorption: 0.0,
            rear_shadow: 0.0,
            room: crate::rooms::Room::Outdoor,
        };
        // Straight ahead: both ears the same, no delay, no filter.
        let [l, r] = ears(&Rendering::CENTRE, &head);
        assert_eq!(l, r);
        assert_eq!((l.gain, l.delay), (1.0, 0.0));
        assert!(l.cutoff.is_infinite());
        // Hard right: the left ear is later (0.66 ms), 6 dB down and low-passed at 1.8 kHz.
        let [l, r] = ears(
            &Rendering {
                gain: 0.5,
                pan: 1.0,
                behind: 0.0,
                distance: 4.0,
            },
            &head,
        );
        assert_eq!((r.gain, r.delay), (0.5, 0.0));
        assert!((l.delay - MAX_ITD).abs() < 1e-12 && (MAX_ITD - 0.000656).abs() < 1e-6);
        assert!((db(l.gain / r.gain) + 6.0).abs() < 1e-9);
        assert!((l.cutoff - 1800.0).abs() < 1e-6);
        // Pan model: today's stereo gains, no delays.
        let pan = Acoustics {
            direction: DirectionModel::Pan,
            air_absorption: 0.0,
            rear_shadow: 0.0,
            room: crate::rooms::Room::Outdoor,
        };
        let [l, r] = ears(
            &Rendering {
                gain: 1.0,
                pan: 1.0,
                behind: 0.0,
                distance: 4.0,
            },
            &pan,
        );
        assert!(l.gain.abs() < 1e-12);
        assert_eq!((r.gain, l.delay, r.delay), (1.0, 0.0, 0.0));
    }

    #[test]
    fn rear_shadow_dulls_sources_behind() {
        let head = Acoustics {
            direction: DirectionModel::Head,
            air_absorption: 0.0,
            rear_shadow: 4000.0,
            room: crate::rooms::Room::Outdoor,
        };
        let from = |behind: f64| {
            ears(
                &Rendering {
                    gain: 1.0,
                    pan: 0.0,
                    behind,
                    distance: 5.0,
                },
                &head,
            )
        };
        // In front: no filter. Straight behind: both ears at 4 kHz. Half way round: in between.
        assert!(from(0.0)[0].cutoff.is_infinite());
        let [l, r] = from(1.0);
        assert_eq!(l, r);
        assert!((l.cutoff - 4000.0).abs() < 1e-6);
        let half = from(0.5)[0].cutoff;
        assert!(half > 4000.0 && half < 20000.0, "{half}");
        // Off: no filter behind.
        let off = Acoustics {
            rear_shadow: 0.0,
            ..head
        };
        assert!(
            ears(
                &Rendering {
                    behind: 1.0,
                    ..Rendering::CENTRE
                },
                &off
            )[0]
            .cutoff
            .is_infinite()
        );
    }

    #[test]
    fn render_tells_behind_from_in_front() {
        let p = DistanceParams::default();
        let facing_east = Some(([0.0, 0.0, 0.0], Orientation::default()));
        let ahead = render(at(5.0, 0.0, 0.0), facing_east, &p);
        let behind = render(at(-5.0, 0.0, 0.0), facing_east, &p);
        let left = render(at(0.0, 5.0, 0.0), facing_east, &p);
        assert_eq!((ahead.behind, behind.behind, left.behind), (0.0, 1.0, 0.0));
        assert_eq!(ahead.gain, behind.gain);
    }

    #[test]
    fn air_absorption_dulls_with_distance() {
        assert!(air_cutoff(0.0, 0.1).is_infinite());
        assert!(air_cutoff(10.0, 0.0).is_infinite());
        // 30 m at 0.1 dB/m: 3 dB down at 8 kHz, i.e. the cut-off is 8 kHz.
        assert!((air_cutoff(30.0, 0.1) - 8000.0).abs() < 20.0);
        assert!(air_cutoff(60.0, 0.1) < air_cutoff(10.0, 0.1));
    }
}
