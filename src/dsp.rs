// Copyright 2026 Simon Annetts/Jak Daniels
// SPDX-License-Identifier: MPL-2.0

//! The mixing itself, without GStreamer: every path (speaker, listener ear) has a gain, a delay and a one-pole
//! low-pass, set once per output buffer from the scene and ramped across it, and each speaker keeps a short history
//! so delayed ears can read older samples.
//!
//! Layout: one row per speaker of `width` values, listener l's left ear at column 2l and right at 2l + 1, the same
//! as the mixer's output frame, so a speaker's row lines up with the frame it adds into.

use crate::scene::PathTable;
use crate::spatial::{MAX_ITD, NO_FILTER_HZ};

pub struct Paths {
    rows: usize,
    width: usize,
    rate: f64,
    /// Samples of history kept per speaker: the longest delay plus room to interpolate.
    history_len: usize,

    // This output buffer's coefficients: start and end (equal unless something changed), and per-frame steps.
    gain_start: Vec<f32>,
    gain_end: Vec<f32>,
    gain_step: Vec<f32>,
    delay_start: Vec<f32>, // samples
    delay_end: Vec<f32>,
    delay_step: Vec<f32>,
    /// One-pole low-pass coefficient (1 = no filter); takes its new value at the buffer start.
    pole: Vec<f32>,
    table: PathTable,

    // Per row: does it ramp this buffer; is it plain gains (no delays, no filters: the fast loop); the columns
    // that are not silent (empty: the speaker is heard by nobody, so its input is skipped).
    ramping: Vec<bool>,
    plain: Vec<bool>,
    span: Vec<(usize, usize)>,

    // Running state: each path's filter memory, each speaker's history, and where in the output (frames since
    // the start) each speaker's next input is expected; anything else is a gap, after which both are cleared.
    filter: Vec<f32>,
    history: Vec<Vec<f32>>,
    next_pos: Vec<u64>,
    /// Output frame at which the current buffer starts, and its length.
    buffer_pos: u64,
    frames: usize,

    work: Vec<f32>,
}

impl Paths {
    pub fn new(rows: usize, width: usize, rate: f64) -> Self {
        let len = rows * width;
        let history_len = (MAX_ITD * rate).ceil() as usize + 2;
        Paths {
            rows,
            width,
            rate,
            history_len,
            gain_start: vec![0.0; len],
            gain_end: vec![0.0; len],
            gain_step: vec![0.0; len],
            delay_start: vec![0.0; len],
            delay_end: vec![0.0; len],
            delay_step: vec![0.0; len],
            pole: vec![1.0; len],
            table: PathTable::new(len),
            ramping: vec![false; rows],
            plain: vec![true; rows],
            span: vec![(0, 0); rows],
            filter: vec![0.0; len],
            history: vec![vec![0.0; history_len]; rows],
            next_pos: vec![u64::MAX; rows],
            buffer_pos: 0,
            frames: 0,
            work: vec![],
        }
    }

    pub fn rate(&self) -> f64 {
        self.rate
    }

    /// Moves on to a new output buffer of `frames` frames. `update` fills in the scene's paths when they changed
    /// (returning the fresh participants, whose paths start at their values instead of ramping), or returns None.
    pub fn next_buffer(
        &mut self,
        frames: usize,
        update: impl FnOnce(&mut PathTable) -> Option<Vec<usize>>,
    ) {
        self.buffer_pos += self.frames as u64;
        self.frames = frames;

        let was_ramping = self.ramping.iter().any(|&r| r);
        if !was_ramping {
            // Start and end are already equal; only a change needs work.
            match update(&mut self.table) {
                Some(fresh) => self.take_table(&fresh),
                None => return,
            }
        } else {
            self.gain_start.copy_from_slice(&self.gain_end);
            self.delay_start.copy_from_slice(&self.delay_end);
            if let Some(fresh) = update(&mut self.table) {
                self.take_table(&fresh);
            }
        }

        let inv_frames = 1.0 / frames.max(1) as f32;
        let width = self.width;
        for row in 0..self.rows {
            let r = row * width..(row + 1) * width;
            let ramping = self.gain_start[r.clone()] != self.gain_end[r.clone()]
                || self.delay_start[r.clone()] != self.delay_end[r.clone()];
            self.ramping[row] = ramping;
            if ramping {
                for c in r.clone() {
                    self.gain_step[c] = (self.gain_end[c] - self.gain_start[c]) * inv_frames;
                    self.delay_step[c] = (self.delay_end[c] - self.delay_start[c]) * inv_frames;
                }
            }
            let audible = |c: usize| {
                self.gain_start[row * width + c] != 0.0 || self.gain_end[row * width + c] != 0.0
            };
            self.span[row] = match (0..width).find(|&c| audible(c)) {
                Some(lo) => (lo, (lo..width).rfind(|&c| audible(c)).unwrap() + 1),
                None => (0, 0),
            };
            let (lo, hi) = self.span[row];
            self.plain[row] = (row * width + lo..row * width + hi).all(|c| {
                !audible(c - row * width)
                    || (self.delay_start[c] == 0.0
                        && self.delay_end[c] == 0.0
                        && self.pole[c] == 1.0)
            });
        }
    }

    /// The scene's new paths become this buffer's end values (start values too, and cleared filters, for the
    /// fresh participants' rows and columns).
    fn take_table(&mut self, fresh: &[usize]) {
        let max_delay = (self.history_len - 2) as f32;
        let rate = self.rate as f32;
        for i in 0..self.gain_end.len() {
            self.gain_end[i] = self.table.gain[i];
            self.delay_end[i] = (self.table.delay[i] * rate).clamp(0.0, max_delay);
            self.pole[i] = pole(self.table.cutoff[i] as f64, self.rate);
        }
        let width = self.width;
        for &n in fresh {
            let row = n * width..(n + 1) * width;
            self.gain_start[row.clone()].copy_from_slice(&self.gain_end[row.clone()]);
            self.delay_start[row.clone()].copy_from_slice(&self.delay_end[row.clone()]);
            self.filter[row].fill(0.0);
            for r in 0..self.rows {
                for c in [r * width + 2 * n, r * width + 2 * n + 1] {
                    self.gain_start[c] = self.gain_end[c];
                    self.delay_start[c] = self.delay_end[c];
                    self.filter[c] = 0.0;
                }
            }
        }
    }

    /// Adds one speaker's mono input into `out`: the output frames from frame `offset` of the current buffer
    /// (`input.len()` frames of `width` channels). `scale` converts the samples to ±1.
    pub fn mix<T: Copy>(
        &mut self,
        row: usize,
        input: &[T],
        scale: f32,
        out: &mut [f32],
        offset: usize,
    ) where
        f32: From<T>,
    {
        let n = input.len();
        let width = self.width;
        let h = self.history_len;
        assert!(out.len() >= n * width);

        // The history, then this input, then one zero (so interpolating at the last sample reads in bounds).
        let pos = self.buffer_pos + offset as u64;
        if self.next_pos[row] != pos {
            self.history[row].fill(0.0);
            self.filter[row * width..(row + 1) * width].fill(0.0);
        }
        self.next_pos[row] = pos + n as u64;
        let mut work = std::mem::take(&mut self.work);
        work.clear();
        work.extend_from_slice(&self.history[row]);
        work.extend(input.iter().map(|&x| f32::from(x) * scale));
        work.push(0.0);

        let (lo, hi) = self.span[row];
        if lo < hi {
            let r = row * width;
            let samples = &work[h..h + n];
            if self.plain[row] {
                mix_plain(
                    samples,
                    out,
                    width,
                    lo,
                    &self.gain_start[r + lo..r + hi],
                    self.ramping[row].then(|| &self.gain_step[r + lo..r + hi]),
                    offset,
                );
            } else {
                let ramping = self.ramping[row];
                fn step(v: &[f32], ramping: bool, cols: std::ops::Range<usize>) -> Option<&[f32]> {
                    ramping.then(|| &v[cols])
                }
                mix_ears(
                    &work,
                    h,
                    n,
                    out,
                    width,
                    lo,
                    Ears {
                        gain: &self.gain_start[r + lo..r + hi],
                        gain_step: step(&self.gain_step, ramping, r + lo..r + hi),
                        delay: &self.delay_start[r + lo..r + hi],
                        delay_step: step(&self.delay_step, ramping, r + lo..r + hi),
                        pole: &self.pole[r + lo..r + hi],
                    },
                    &mut self.filter[r + lo..r + hi],
                    offset,
                );
            }
        }

        // Keep the last history_len samples.
        let end = h + n;
        self.history[row].copy_from_slice(&work[end - h..end]);
        self.work = work;
    }
}

/// The one-pole low-pass coefficient for a cut-off (1 = no filter).
fn pole(cutoff: f64, rate: f64) -> f32 {
    if !cutoff.is_finite() || cutoff >= NO_FILTER_HZ || cutoff >= 0.45 * rate {
        1.0
    } else {
        (1.0 - (-2.0 * std::f64::consts::PI * cutoff / rate).exp()) as f32
    }
}

/// Plain gains for every column: per sample one multiply-add over a contiguous run of the output frame, which
/// vectorises. Gains ramp as start + step × (k + 1) at output-buffer frame k (`first` is this call's first).
fn mix_plain(
    input: &[f32],
    out: &mut [f32],
    width: usize,
    lo: usize,
    start: &[f32],
    step: Option<&[f32]>,
    first: usize,
) {
    let hi = lo + start.len();
    match step {
        None => {
            for (frame, &x) in out.chunks_exact_mut(width).zip(input) {
                for (o, &g) in frame[lo..hi].iter_mut().zip(start) {
                    *o += x * g;
                }
            }
        }
        Some(step) => {
            for (i, (frame, &x)) in out.chunks_exact_mut(width).zip(input).enumerate() {
                let k = (first + i + 1) as f32;
                for ((o, &g), &d) in frame[lo..hi].iter_mut().zip(start).zip(step) {
                    *o += x * (g + d * k);
                }
            }
        }
    }
}

/// One speaker's ears for columns lo..hi: gains, delays (samples) and filter poles, and their per-frame steps
/// when ramping.
struct Ears<'a> {
    gain: &'a [f32],
    gain_step: Option<&'a [f32]>,
    delay: &'a [f32],
    delay_step: Option<&'a [f32]>,
    pole: &'a [f32],
}

/// Every ear with a delay or a filter: per output frame, across columns lo..hi, the input read that many samples
/// late (linearly interpolated, so a changing delay glides), low-passed, scaled and added. The filter feeds back
/// in time, so the loop runs across the ears of one frame (independent of each other), not along time. `work`
/// holds `h` samples of history, then the `n` input samples, then a zero; `state` is the filters' memory.
#[allow(clippy::too_many_arguments)]
fn mix_ears(
    work: &[f32],
    h: usize,
    n: usize,
    out: &mut [f32],
    width: usize,
    lo: usize,
    ears: Ears,
    state: &mut [f32],
    first: usize,
) {
    let len = ears.gain.len();
    let zeros = vec![0.0; len];
    let gain_step = ears.gain_step.unwrap_or(&zeros);

    let Some(delay_step) = ears.delay_step else {
        // Delays fixed for this buffer (the usual case): each ear reads `back` samples behind now, `frac` of the
        // way to the next one, worked out once.
        let back: Vec<usize> = ears.delay.iter().map(|&d| d.ceil() as usize).collect();
        let frac: Vec<f32> = ears
            .delay
            .iter()
            .zip(&back)
            .map(|(&d, &b)| b as f32 - d)
            .collect();
        // Every read is in bounds: base − back ≥ h − max delay ≥ 0, and base + 1 ≤ h + n < work.len().
        assert!(back.iter().all(|&b| b <= h) && work.len() > h + n);
        for (k, frame) in out.chunks_exact_mut(width).take(n).enumerate() {
            let t = (first + k + 1) as f32;
            let base = h + k;
            let frame = &mut frame[lo..lo + len];
            let columns = frame
                .iter_mut()
                .zip(state.iter_mut())
                .zip(back.iter().zip(&frac))
                .zip(ears.pole.iter().zip(ears.gain.iter().zip(gain_step)));
            for (((o, y), (&b, &f)), (&a, (&g, &gs))) in columns {
                // SAFETY: the bounds are checked by the assertion above.
                let (x0, x1) = unsafe {
                    let i = base - b;
                    (*work.get_unchecked(i), *work.get_unchecked(i + 1))
                };
                let x = x0 + f * (x1 - x0);
                *y += a * (x - *y);
                *o += (g + gs * t) * *y;
            }
        }
        return;
    };

    // Delays changing: worked out per frame, so they glide.
    for (k, frame) in out.chunks_exact_mut(width).take(n).enumerate() {
        let t = (first + k + 1) as f32;
        let now = (h + k) as f32;
        let frame = &mut frame[lo..lo + len];
        for j in 0..len {
            let at = now - (ears.delay[j] + delay_step[j] * t);
            let i = at as usize; // at ≥ 0: delays never exceed the history
            let f = at - i as f32;
            let x = work[i] + f * (work[i + 1] - work[i]);
            let y = state[j] + ears.pole[j] * (x - state[j]);
            state[j] = y;
            frame[j] += (ears.gain[j] + gain_step[j] * t) * y;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: f64 = 48_000.0;

    /// One speaker (row 0) heard by listener 1 (columns 2, 3) through the given left and right ears.
    fn one_path(
        left: (f32, f32, f32),
        right: (f32, f32, f32),
    ) -> impl FnOnce(&mut PathTable) -> Option<Vec<usize>> {
        move |t: &mut PathTable| {
            for (c, (g, d, f)) in [(2, left), (3, right)] {
                t.gain[c] = g;
                t.delay[c] = d;
                t.cutoff[c] = f;
            }
            Some(vec![0, 1])
        }
    }

    fn run(paths: &mut Paths, input: &[f32]) -> Vec<f32> {
        let mut out = vec![0.0; input.len() * 4];
        paths.mix(0, input, 1.0, &mut out, 0);
        out
    }

    fn column(out: &[f32], c: usize) -> Vec<f32> {
        out.as_chunks::<4>().0.iter().map(|f| f[c]).collect()
    }

    #[test]
    fn plain_gains_take_the_fast_loop() {
        let mut p = Paths::new(2, 4, RATE);
        p.next_buffer(
            8,
            one_path((0.5, 0.0, f32::INFINITY), (0.25, 0.0, f32::INFINITY)),
        );
        assert!(p.plain[0]);
        let out = run(&mut p, &[1.0; 8]);
        assert_eq!(column(&out, 2), [0.5; 8]);
        assert_eq!(column(&out, 3), [0.25; 8]);
        assert_eq!(column(&out, 0), [0.0; 8]); // the speaker does not hear themselves
    }

    #[test]
    fn a_delayed_ear_reads_older_samples_across_buffers() {
        let mut p = Paths::new(2, 4, RATE);
        // Right ear 2.5 samples late.
        p.next_buffer(
            4,
            one_path(
                (1.0, 0.0, f32::INFINITY),
                (1.0, 2.5 / RATE as f32, f32::INFINITY),
            ),
        );
        assert!(!p.plain[0]);
        let out = run(&mut p, &[0.0, 0.0, 0.0, 1.0]); // an impulse at the last sample
        assert_eq!(column(&out, 2), [0.0, 0.0, 0.0, 1.0]);
        assert_eq!(column(&out, 3), [0.0; 4]);
        // The next buffer: the delayed impulse arrives halfway between samples 1 and 2 (from history).
        p.next_buffer(4, |_| None);
        let mut out = vec![0.0; 16];
        p.mix(0, &[0.0f32; 4], 1.0, &mut out, 0);
        assert_eq!(column(&out, 3), [0.0, 0.5, 0.5, 0.0]);
    }

    #[test]
    fn a_gap_clears_the_history() {
        let mut p = Paths::new(2, 4, RATE);
        p.next_buffer(
            4,
            one_path(
                (1.0, 0.0, f32::INFINITY),
                (1.0, 2.0 / RATE as f32, f32::INFINITY),
            ),
        );
        run(&mut p, &[0.0, 0.0, 0.0, 1.0]);
        p.next_buffer(4, |_| None);
        p.next_buffer(4, |_| None); // this input skipped a buffer
        let mut out = vec![0.0; 16];
        p.mix(0, &[0.0f32; 4], 1.0, &mut out, 0);
        assert_eq!(column(&out, 3), [0.0; 4]);
    }

    #[test]
    fn the_low_pass_is_a_one_pole() {
        let mut p = Paths::new(2, 4, RATE);
        p.next_buffer(64, one_path((1.0, 0.0, 1000.0), (1.0, 0.0, f32::INFINITY)));
        let out = run(&mut p, &[1.0; 64]);
        let a = 1.0 - (-2.0 * std::f64::consts::PI * 1000.0 / RATE).exp();
        let left = column(&out, 2);
        for (k, &y) in left.iter().enumerate() {
            let expected = 1.0 - (1.0 - a).powi(k as i32 + 1);
            assert!((y as f64 - expected).abs() < 1e-5, "{k}: {y} vs {expected}");
        }
        assert_eq!(column(&out, 3), [1.0; 64]);
    }

    #[test]
    fn changes_ramp_gain_and_delay_across_one_buffer() {
        let mut p = Paths::new(2, 4, RATE);
        p.next_buffer(
            4,
            one_path((1.0, 0.0, f32::INFINITY), (1.0, 0.0, f32::INFINITY)),
        );
        run(&mut p, &[1.0; 4]);
        // The left ear goes to 0.5 over the next buffer, frame k at (k + 1) / 4 of the way.
        p.next_buffer(4, |t| {
            t.gain[2] = 0.5;
            Some(vec![])
        });
        let out = run(&mut p, &[1.0; 4]);
        assert_eq!(column(&out, 2), [0.875, 0.75, 0.625, 0.5]);
        p.next_buffer(4, |_| None);
        let out = run(&mut p, &[1.0; 4]);
        assert_eq!(column(&out, 2), [0.5; 4]);
        assert!(!p.ramping[0]);
    }
}
