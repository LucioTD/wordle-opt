//! Cheap first-guess metrics over a candidate set.

use crate::matrix::Matrix;
use crate::pattern::{ALL_GREEN, NPAT};
use rayon::prelude::*;
use serde::Serialize;

#[derive(Clone, Serialize, Debug)]
pub struct Stats {
    pub guess: usize,
    pub word: String,
    /// Shannon entropy of the pattern distribution, bits.
    pub entropy: f64,
    /// Expected size of the remaining set (sum |B|^2 / N).
    pub exp_size: f64,
    /// Largest bucket.
    pub worst: usize,
    /// Number of non-empty buckets (including the all-green one).
    pub buckets: usize,
    /// Exact P(solved within 2) under optimal follow-up = buckets / N.
    pub p_le2: f64,
    /// Exact P(solved within 3) under follow-up that maximizes it (shortlist only).
    pub p_le3: Option<f64>,
    pub is_candidate: bool,
}

/// Bucket sizes of guess g over set s.
#[inline]
pub fn histogram(m: &Matrix, g: usize, s: &[u16], h: &mut [u32; NPAT]) {
    h.fill(0);
    let row = m.row(g);
    for &a in s {
        h[row[a as usize] as usize] += 1;
    }
}

pub fn stats_for(m: &Matrix, g: usize, s: &[u16], word: &str) -> Stats {
    let mut h = [0u32; NPAT];
    histogram(m, g, s, &mut h);
    let n = s.len() as f64;
    let (mut ent, mut sq, mut worst, mut nb) = (0.0, 0.0, 0usize, 0usize);
    for &c in h.iter() {
        if c > 0 {
            let p = c as f64 / n;
            ent -= p * p.log2();
            sq += (c as f64) * (c as f64);
            worst = worst.max(c as usize);
            nb += 1;
        }
    }
    Stats {
        guess: g,
        word: word.to_string(),
        entropy: ent,
        exp_size: sq / n,
        worst,
        buckets: nb,
        p_le2: nb as f64 / n,
        p_le3: None,
        is_candidate: h[ALL_GREEN as usize] > 0,
    }
}

/// Group set `s` by the pattern of guess g. Returns (pattern, members) pairs.
pub fn partition(m: &Matrix, g: usize, s: &[u16]) -> Vec<(u8, Vec<u16>)> {
    let mut h = [0u32; NPAT];
    histogram(m, g, s, &mut h);
    let mut idx = [u32::MAX; NPAT];
    let mut out: Vec<(u8, Vec<u16>)> = Vec::new();
    for p in 0..NPAT {
        if h[p] > 0 {
            idx[p] = out.len() as u32;
            out.push((p as u8, Vec::with_capacity(h[p] as usize)));
        }
    }
    let row = m.row(g);
    for &a in s {
        out[idx[row[a as usize] as usize] as usize].1.push(a);
    }
    out
}

/// Max number of answers in bucket `b` that can be solved within ONE or TWO more
/// guesses: choose guess g to maximize [g in b] + (#non-green parts of b under g).
/// Returns the max count over all guesses (exhaustive).
pub fn best_two_more(m: &Matrix, b: &[u16], answer_guess: &[usize]) -> usize {
    if b.len() <= 2 {
        return b.len();
    }
    let mut best = 0;
    let mut seen = [0u32; NPAT];
    let mut stamp = 0u32;
    // Every allowed guess is tried, so the result is exact.
    for g in 0..m.ng {
        stamp += 1;
        let row = m.row(g);
        let mut parts = 0usize;
        let mut green = 0usize;
        for &a in b {
            let p = row[a as usize];
            if p == ALL_GREEN {
                green = 1;
            } else if seen[p as usize] != stamp {
                seen[p as usize] = stamp;
                parts += 1;
            }
        }
        best = best.max(parts + green);
        if best == b.len() {
            break;
        }
    }
    let _ = answer_guess;
    best
}

/// Exact P(solved within 3) for opener g with the best possible follow-up.
pub fn p_le3(m: &Matrix, g: usize, s: &[u16], answer_guess: &[usize]) -> f64 {
    let mut solved = 0usize;
    for (p, b) in partition(m, g, s) {
        if p == ALL_GREEN {
            solved += 1;
        } else {
            solved += best_two_more(m, &b, answer_guess);
        }
    }
    solved as f64 / s.len() as f64
}

pub fn rank_all(m: &Matrix, s: &[u16], words: &[String]) -> Vec<Stats> {
    (0..m.ng).into_par_iter().map(|g| stats_for(m, g, s, &words[g])).collect()
}
