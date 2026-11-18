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

/// How one source sounds to one listener: a level (0 = silent, out of the mix) and a pan (−1 left … 1 right).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rendering {
    pub gain: f64,
    pub pan: f64,
}

impl Rendering {
    pub const SILENT: Rendering = Rendering {
        gain: 0.0,
        pan: 0.0,
    };

    /// Full volume, centred: a participant without a position.
    pub const CENTRE: Rendering = Rendering {
        gain: 1.0,
        pan: 0.0,
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
    let pan = if horizontal < 1e-6 {
        0.0
    } else {
        -left / horizontal
    };
    Rendering { gain, pan }
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
        }
        .stereo();
        assert!(close(l, 0.0) && close(r, 1.0));
        let (l, r) = Rendering {
            gain: 0.5,
            pan: -1.0,
        }
        .stereo();
        assert!(close(l, 0.5) && close(r, 0.0));
        let (l, r) = Rendering {
            gain: 1.0,
            pan: 0.5,
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
}
