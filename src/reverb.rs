// Copyright 2026 Simon Annetts/Jak Daniels
// SPDX-License-Identifier: MPL-2.0

//! Late reverb: a feedback delay network (eight delay lines whose outputs are mixed by a Hadamard matrix and fed
//! back), one per voice, so that each listener's mix can leave their own voice out exactly. Each line's feedback
//! has a one-pole low-pass, set so that the bass dies away over the room's decay time and the treble faster. The
//! left ear hears half the lines and the right ear the other half, so the tail is wide.

use crate::rooms::Reverb;

const LINES: usize = 8;
/// Line lengths against the room's size: spread out and (once rounded to primes) without common factors, so the
/// echoes do not pile up into a pitch.
const RATIOS: [f64; LINES] = [0.62, 0.71, 0.83, 0.94, 1.05, 1.17, 1.29, 1.39];
/// How the input is fed into the lines.
const SIGNS: [f32; LINES] = [1.0, -1.0, 1.0, 1.0, -1.0, 1.0, -1.0, -1.0];
/// Keeps the Hadamard mix lossless.
const SCALE: f32 = 0.353_553_4; // 1 / √8
/// Below this a decaying value is flushed to zero, so a fading tail never turns into slow denormal arithmetic.
const FLUSH: f32 = 1e-15;

/// A room's reverb in samples and coefficients, shared by every voice.
#[derive(Debug, Clone)]
pub struct Network {
    len: [usize; LINES],
    /// Where each line starts in a voice's buffer.
    offset: [usize; LINES],
    total: usize,
    /// Per line, its feedback filter y = b0·x + p·y: the bass gain (one trip round the line) times 1 − p, and the
    /// pole.
    b0: [f32; LINES],
    p: [f32; LINES],
    /// The gain into the network that gives the preset's level, and that times the element's adjustment.
    base_input: f32,
    input: f32,
}

/// One voice's reverb: its lines, where each is read and written next, and their filters' memory.
#[derive(Debug, Clone, Default)]
pub struct State {
    lines: Vec<f32>,
    at: [usize; LINES],
    damp: [f32; LINES],
}

impl State {
    pub fn clear(&mut self) {
        self.lines.fill(0.0);
        self.damp = [0.0; LINES];
    }
}

impl Network {
    pub fn new(reverb: &Reverb, rate: f64) -> Self {
        let mut len = [0; LINES];
        for (i, ratio) in RATIOS.iter().enumerate() {
            let mut d = next_prime((reverb.size_ms / 1000.0 * rate * ratio).round() as usize);
            while len[..i].contains(&d) {
                d = next_prime(d + 1);
            }
            len[i] = d;
        }
        let mut offset = [0; LINES];
        for i in 1..LINES {
            offset[i] = offset[i - 1] + len[i - 1];
        }
        let total = offset[LINES - 1] + len[LINES - 1];

        // One trip round line i loses 60 dB × (its length / the decay time), in the bass and in the treble; a
        // one-pole low-pass with those gains at DC and at the top of the band does both.
        let trip = |d: usize, decay: f64| 10f64.powf(-3.0 * d as f64 / (decay * rate));
        let mut b0 = [0.0; LINES];
        let mut p = [0.0; LINES];
        for i in 0..LINES {
            let bass = trip(len[i], reverb.decay_s);
            let treble = trip(len[i], reverb.treble_decay_s.min(reverb.decay_s));
            let pole = (bass - treble) / (bass + treble);
            p[i] = pole as f32;
            b0[i] = (bass * (1.0 - pole)) as f32;
        }

        // The tail's energy for an input of energy 1 (in the bass, each ear): the network holds what went in and
        // loses it at 60 dB per decay time; the mix spreads it evenly over every stored sample, so the lines give
        // out a 1 / (mean length) share of it each sample, and each ear hears 1 / LINES of that. Summed:
        // input² × decay × rate / (6 ln 10 × mean length).
        let mean = total as f64 / LINES as f64;
        let energy = 10f64.powf(reverb.level_db / 10.0);
        let base_input =
            (energy * 6.0 * std::f64::consts::LN_10 * mean / (reverb.decay_s * rate)).sqrt() as f32;

        Network {
            len,
            offset,
            total,
            b0,
            p,
            base_input,
            input: base_input,
        }
    }

    /// Scales the level (1 = the preset's).
    pub fn set_level(&mut self, factor: f32) {
        self.input = self.base_input * factor;
    }

    /// A silent voice's state.
    pub fn state(&self) -> State {
        State {
            lines: vec![0.0; self.total],
            ..State::default()
        }
    }

    /// Feeds `input` into a voice's network and adds its tail to `left` and `right`.
    pub fn run(&self, state: &mut State, input: &[f32], left: &mut [f32], right: &mut [f32]) {
        debug_assert_eq!(state.lines.len(), self.total);
        for ((&x, l), r) in input.iter().zip(left).zip(right) {
            let mut out = [0.0; LINES];
            let mut back = [0.0; LINES];
            for i in 0..LINES {
                out[i] = state.lines[self.offset[i] + state.at[i]];
                let y = self.b0[i] * out[i] + self.p[i] * state.damp[i];
                let y = if y.abs() < FLUSH { 0.0 } else { y };
                state.damp[i] = y;
                back[i] = y;
            }
            hadamard(&mut back);
            let fed = self.input * x;
            for i in 0..LINES {
                state.lines[self.offset[i] + state.at[i]] = back[i] * SCALE + SIGNS[i] * fed;
                state.at[i] += 1;
                if state.at[i] == self.len[i] {
                    state.at[i] = 0;
                }
            }
            *l += 0.5 * (out[0] - out[2] + out[4] - out[6]);
            *r += 0.5 * (out[1] - out[3] + out[5] - out[7]);
        }
    }

    /// When the tail starts: the shortest line (samples).
    #[cfg(test)]
    pub fn shortest(&self) -> usize {
        *self.len.iter().min().unwrap()
    }
}

/// The 8-point Hadamard transform, unscaled.
#[inline(always)]
fn hadamard(v: &mut [f32; LINES]) {
    let mut h = 1;
    while h < LINES {
        for i in (0..LINES).step_by(2 * h) {
            for j in i..i + h {
                let (a, b) = (v[j], v[j + h]);
                v[j] = a + b;
                v[j + h] = a - b;
            }
        }
        h *= 2;
    }
}

fn next_prime(mut n: usize) -> usize {
    n = n.max(2);
    while !(2..)
        .take_while(|d| d * d <= n)
        .all(|d| !n.is_multiple_of(d))
    {
        n += 1;
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rooms::Room;

    const RATE: f64 = 48_000.0;

    /// A voice's impulse response, `seconds` long.
    fn impulse(net: &Network, seconds: f64) -> (Vec<f32>, Vec<f32>) {
        let n = (seconds * RATE) as usize;
        let mut input = vec![0.0; n];
        input[0] = 1.0;
        let (mut l, mut r) = (vec![0.0; n], vec![0.0; n]);
        net.run(&mut net.state(), &input, &mut l, &mut r);
        (l, r)
    }

    fn energy(x: &[f32]) -> f64 {
        x.iter().map(|&v| (v as f64).powi(2)).sum()
    }

    fn db(x: f64) -> f64 {
        10.0 * x.log10()
    }

    #[test]
    fn line_lengths_are_distinct_primes() {
        for room in [Room::Small, Room::Hall, Room::Cave] {
            let net = Network::new(&room.preset().reverb.unwrap(), RATE);
            for (i, &d) in net.len.iter().enumerate() {
                assert_eq!(next_prime(d), d);
                assert!(!net.len[..i].contains(&d));
            }
        }
    }

    #[test]
    fn silence_until_the_shortest_line_then_a_tail_in_both_ears() {
        let net = Network::new(&Room::Hall.preset().reverb.unwrap(), RATE);
        let (l, r) = impulse(&net, 0.5);
        let first = net.shortest();
        assert!(l[..first].iter().chain(&r[..first]).all(|&v| v == 0.0));
        assert!(energy(&l[first..]) > 0.0 && energy(&r[first..]) > 0.0);
        // The ears hear different lines: alike in level, not in detail.
        let cross: f64 = l.iter().zip(&r).map(|(&a, &b)| a as f64 * b as f64).sum();
        assert!(cross.abs() < 0.3 * (energy(&l) * energy(&r)).sqrt());
    }

    #[test]
    fn level_and_decay_time_match_the_preset() {
        // Without treble damping, so the level and the decay are the bass ones for every sample.
        let hall = Room::Hall.preset().reverb.unwrap();
        let reverb = Reverb {
            treble_decay_s: hall.decay_s,
            ..hall
        };
        let net = Network::new(&reverb, RATE);
        let (l, r) = impulse(&net, 4.0);
        for ear in [&l, &r] {
            let total = db(energy(ear));
            assert!((total - reverb.level_db).abs() < 1.5, "level {total}");
            // Energy left after t falls 60 dB per decay time: compare a quarter and three quarters of it (−30 dB).
            let after = |t: f64| energy(&ear[(t * reverb.decay_s * RATE) as usize..]);
            let fall = db(after(0.25)) - db(after(0.75));
            assert!((fall - 30.0).abs() < 3.0, "fall {fall}");
        }
    }

    #[test]
    fn the_treble_dies_away_sooner() {
        let net = Network::new(&Room::Hall.preset().reverb.unwrap(), RATE);
        let (l, _) = impulse(&net, 1.5);
        // High-passed by a first difference: its share of the energy shrinks along the tail.
        let treble_share = |x: &[f32]| {
            let hi: f64 = x.windows(2).map(|w| ((w[1] - w[0]) as f64).powi(2)).sum();
            hi / energy(x)
        };
        let early = treble_share(&l[..(0.3 * RATE) as usize]);
        let late = treble_share(&l[(0.8 * RATE) as usize..]);
        assert!(late < 0.5 * early, "{early} {late}");
    }

    #[test]
    fn a_tail_fades_to_exact_zero() {
        let net = Network::new(&Room::Small.preset().reverb.unwrap(), RATE);
        let (l, r) = impulse(&net, 8.0);
        let end = l.len() - 4800;
        assert!(l[end..].iter().chain(&r[end..]).all(|&v| v == 0.0));
    }
}
