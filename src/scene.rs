// Copyright 2026 Simon Annetts/Jak Daniels
// SPDX-License-Identifier: MPL-2.0

//! The participants and their settings, shared between the pads (whose properties write it, from any thread) and
//! the mixer (which turns it into gains once per output buffer, when something changed). The lock around it is
//! only held to read or write settings, never while mixing.

use std::collections::BTreeMap;

use crate::spatial::{Acoustics, DistanceParams, Orientation, ears, render};

/// Largest gain (and per-listener trim) accepted: +20 dB.
pub const MAX_GAIN: f64 = 10.0;

#[derive(Debug, Clone)]
pub struct Participant {
    /// Has a sink pad.
    pub active: bool,
    /// Its output is linked, so it is worth mixing.
    pub listening: bool,
    /// Joined, or started listening, since the gains were last computed: its paths start at their values
    /// instead of ramping from silence.
    pub fresh: bool,

    /// Has a position; without one, the voice is heard centred at full volume.
    pub positioned: bool,
    pub position: [f64; 3],
    pub orientation: Orientation,

    /// Hears from its own pose (below) instead of the speaking one.
    pub listener_separate: bool,
    pub listener_position: [f64; 3],
    pub listener_orientation: Orientation,

    /// Applies to everyone hearing this voice.
    pub gain: f64,
    pub mute: bool,
    /// As a listener: a gain per speaker (pad number), on top of everything else.
    pub trims: BTreeMap<u32, f64>,
}

impl Default for Participant {
    fn default() -> Self {
        Participant {
            active: false,
            listening: false,
            fresh: false,
            positioned: false,
            position: [0.0; 3],
            orientation: Orientation::default(),
            listener_separate: false,
            listener_position: [0.0; 3],
            listener_orientation: Orientation::default(),
            gain: 1.0,
            mute: false,
            trims: BTreeMap::new(),
        }
    }
}

impl Participant {
    fn speaker(&self) -> Option<[f64; 3]> {
        self.positioned.then_some(self.position)
    }

    fn listener(&self) -> Option<([f64; 3], Orientation)> {
        if self.listener_separate {
            Some((self.listener_position, self.listener_orientation))
        } else {
            self.positioned.then_some((self.position, self.orientation))
        }
    }
}

#[derive(Debug)]
pub struct Scene {
    pub participants: Vec<Participant>,
    pub params: DistanceParams,
    pub acoustics: Acoustics,
    /// Something changed since the gains were last computed.
    pub dirty: bool,
}

impl Scene {
    pub fn new(max_participants: usize) -> Self {
        Scene {
            participants: vec![Participant::default(); max_participants],
            params: DistanceParams::default(),
            acoustics: Acoustics::default(),
            dirty: true,
        }
    }

    /// Number of output channels: two per participant.
    pub fn width(&self) -> usize {
        2 * self.participants.len()
    }

    pub fn join(&mut self, n: usize) {
        self.participants[n] = Participant {
            active: true,
            fresh: true,
            ..Participant::default()
        };
        self.dirty = true;
    }

    pub fn leave(&mut self, n: usize) {
        self.participants[n] = Participant::default();
        // A later participant given the same number must not inherit what others set for this one.
        for p in &mut self.participants {
            p.trims.remove(&(n as u32));
        }
        self.dirty = true;
    }

    pub fn set_listening(&mut self, n: usize, listening: bool) {
        let p = &mut self.participants[n];
        if p.active && p.listening != listening {
            p.listening = listening;
            p.fresh |= listening;
            self.dirty = true;
        }
    }

    /// Writes every path into `paths` (one row per speaker of `width()` values, listener l's ears at 2l and 2l + 1)
    /// and returns the participants that are fresh (clearing the flag).
    pub fn compute(&mut self, paths: &mut PathTable) -> Vec<usize> {
        let width = self.width();
        assert_eq!(paths.gain.len(), self.participants.len() * width);
        paths.clear();

        for (s, speaker) in self.participants.iter().enumerate() {
            if !speaker.active || speaker.mute || speaker.gain <= 0.0 {
                continue;
            }
            for (l, listener) in self.participants.iter().enumerate() {
                if l == s || !listener.active || !listener.listening {
                    continue;
                }
                let trim = listener.trims.get(&(s as u32)).copied().unwrap_or(1.0);
                if trim <= 0.0 {
                    continue;
                }
                let mut r = render(speaker.speaker(), listener.listener(), &self.params);
                r.gain *= speaker.gain * trim;
                for (ear, e) in ears(&r, &self.acoustics).into_iter().enumerate() {
                    let i = s * width + 2 * l + ear;
                    paths.gain[i] = e.gain as f32;
                    paths.delay[i] = e.delay as f32;
                    paths.cutoff[i] = e.cutoff as f32;
                }
            }
        }

        self.dirty = false;
        let mut fresh = vec![];
        for (n, p) in self.participants.iter_mut().enumerate() {
            if p.fresh {
                p.fresh = false;
                fresh.push(n);
            }
        }
        fresh
    }
}

/// Every path's ears, one row per speaker of `Scene::width()` values: gain, delay (seconds) and low-pass cut-off
/// (Hz; infinite = none).
#[derive(Debug, Clone)]
pub struct PathTable {
    pub gain: Vec<f32>,
    pub delay: Vec<f32>,
    pub cutoff: Vec<f32>,
}

impl PathTable {
    pub fn new(len: usize) -> Self {
        PathTable {
            gain: vec![0.0; len],
            delay: vec![0.0; len],
            cutoff: vec![f32::INFINITY; len],
        }
    }

    fn clear(&mut self) {
        self.gain.fill(0.0);
        self.delay.fill(0.0);
        self.cutoff.fill(f32::INFINITY);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pair(paths: &PathTable, width: usize, s: usize, l: usize) -> (f32, f32) {
        (
            paths.gain[s * width + 2 * l],
            paths.gain[s * width + 2 * l + 1],
        )
    }

    #[test]
    fn mix_minus_trims_mute_and_listening() {
        let mut scene = Scene::new(4);
        for n in 0..3 {
            scene.join(n);
            scene.set_listening(n, true);
        }
        let width = scene.width();
        let mut gains = PathTable::new(4 * width);
        assert_eq!(scene.compute(&mut gains), [0, 1, 2]);

        // Nobody hears themselves; unpositioned is centred, full volume.
        assert_eq!(pair(&gains, width, 0, 0), (0.0, 0.0));
        assert_eq!(pair(&gains, width, 0, 1), (1.0, 1.0));
        assert_eq!(pair(&gains, width, 1, 2), (1.0, 1.0));
        // Participant 3 has not joined.
        assert_eq!(pair(&gains, width, 0, 3), (0.0, 0.0));

        scene.participants[1].trims.insert(0, 0.5);
        scene.participants[2].mute = true;
        scene.participants[0].gain = 2.0;
        scene.set_listening(1, false);
        assert!(scene.compute(&mut gains).is_empty());
        assert_eq!(pair(&gains, width, 0, 1), (0.0, 0.0)); // not listening
        assert_eq!(pair(&gains, width, 0, 2), (2.0, 2.0)); // gain
        assert_eq!(pair(&gains, width, 2, 0), (0.0, 0.0)); // muted

        scene.set_listening(1, true);
        assert_eq!(scene.compute(&mut gains), [1]);
        assert_eq!(pair(&gains, width, 0, 1), (1.0, 1.0)); // 2 × 0.5

        // Leaving clears what others set for that number.
        scene.leave(0);
        assert!(!scene.participants[1].trims.contains_key(&0));
    }

    #[test]
    fn positions() {
        let mut scene = Scene::new(2);
        scene.acoustics.direction = crate::spatial::DirectionModel::Pan;
        scene.acoustics.air_absorption = 0.0;
        for n in 0..2 {
            scene.join(n);
            scene.set_listening(n, true);
            scene.participants[n].positioned = true;
        }
        // 1 is 4 m to the left of 0 (who faces +x).
        scene.participants[1].position = [0.0, 4.0, 0.0];
        let width = scene.width();
        let mut gains = PathTable::new(2 * width);
        scene.compute(&mut gains);
        assert_eq!(pair(&gains, width, 1, 0), (0.5, 0.0));

        // 0 listens from elsewhere: right next to 1, facing −x, so 1 is on their right.
        let p = &mut scene.participants[0];
        p.listener_separate = true;
        p.listener_position = [0.0, 3.0, 0.0];
        p.listener_orientation.yaw = 180.0;
        scene.compute(&mut gains);
        let (l, r) = pair(&gains, width, 1, 0);
        assert!(l.abs() < 1e-6 && (r - 1.0).abs() < 1e-6, "{l} {r}");
    }
}
