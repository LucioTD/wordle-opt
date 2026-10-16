//! Exhaustive FIXED-PAIR search.
//!
//! Score of a pair (f, s) = number of distinct joint (pattern_f, pattern_s) buckets.
//! With a fixed pair, guess 3 solves an answer with probability 1/|bucket|, so
//! P(solved within 3) = buckets / N exactly. The pair score is symmetric, so only
//! unordered pairs are scanned.
//!
//! Safe pruning (never discards a pair that could enter the top K):
//!   joint(f, s) = sum over f-buckets B of distinct_s(B)
//!   distinct_s(B) <= min(|B|, nb(s))
//! f-buckets are processed largest first; after each one the exact count so far
//! plus the bound for the rest is compared with the current K-th best.

use crate::matrix::Matrix;
use crate::metrics::histogram;
use crate::pattern::NPAT;
use rayon::prelude::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

#[derive(Clone, Debug)]
pub struct PairScore {
    pub f: usize,
    pub s: usize,
    pub buckets: usize,
}

pub struct PairStats {
    pub evaluated_full: u64,
    pub pruned_early: u64,
    pub pruned_mid: u64,
}

pub fn search(m: &Matrix, set: &[u16], top_k: usize) -> (Vec<PairScore>, PairStats) {
    let ng = m.ng;
    // nb(g) for all guesses and an order by nb descending (good pairs found early).
    let nb: Vec<usize> = (0..ng)
        .into_par_iter()
        .map(|g| {
            let mut h = [0u32; NPAT];
            histogram(m, g, set, &mut h);
            h.iter().filter(|&&c| c > 0).count()
        })
        .collect();
    let mut order: Vec<usize> = (0..ng).collect();
    order.sort_by_key(|&g| std::cmp::Reverse(nb[g]));
    let rank_of: Vec<usize> = {
        let mut r = vec![0; ng];
        for (i, &g) in order.iter().enumerate() {
            r[g] = i;
        }
        r
    };

    let threshold = AtomicUsize::new(0); // K-th best so far (strictly must exceed)
    let top: Mutex<Vec<PairScore>> = Mutex::new(Vec::new());
    let (c_full, c_early, c_mid) = (AtomicUsize::new(0), AtomicUsize::new(0), AtomicUsize::new(0));

    order.par_iter().enumerate().for_each(|(fi, &f)| {
        // Group the set by f's pattern, largest buckets first.
        let row_f = m.row(f);
        let mut groups: Vec<Vec<u16>> = vec![Vec::new(); NPAT];
        for &a in set {
            groups[row_f[a as usize] as usize].push(a);
        }
        let mut buckets: Vec<Vec<u16>> = groups.into_iter().filter(|v| !v.is_empty()).collect();
        buckets.sort_by_key(|v| std::cmp::Reverse(v.len()));
        let sizes: Vec<usize> = buckets.iter().map(|v| v.len()).collect();
        let singles = sizes.iter().filter(|&&x| x == 1).count();
        let multi = sizes.len() - singles;
        let mut local: Vec<PairScore> = Vec::new();
        let (mut lf, mut le, mut lm) = (0usize, 0usize, 0usize);

        for &s in &order[fi + 1..] {
            let t = threshold.load(Ordering::Relaxed);
            let nbs = nb[s];
            // Whole-pair bound.
            let ub: usize = sizes.iter().map(|&x| x.min(nbs)).sum();
            if ub <= t {
                le += 1;
                // Since order is by nb descending, every later s has nb <= nbs,
                // so its bound is <= this one: stop the inner loop entirely.
                le += order.len() - rank_of[s] - 1;
                break;
            }
            let row_s = m.row(s);
            let mut count = singles; // singleton f-buckets always stay singletons
            let mut rest_ub = ub - singles;
            let mut aborted = false;
            for b in &buckets[..multi] {
                let mut mask = [0u64; 4];
                for &a in b {
                    let p = row_s[a as usize];
                    mask[(p >> 6) as usize] |= 1u64 << (p & 63);
                }
                let k: u32 = mask.iter().map(|w| w.count_ones()).sum();
                count += k as usize;
                rest_ub -= b.len().min(nbs);
                if count + rest_ub <= t {
                    aborted = true;
                    break;
                }
            }
            if aborted {
                lm += 1;
                continue;
            }
            lf += 1;
            if count > t {
                local.push(PairScore { f, s, buckets: count });
                if local.len() >= 4 * top_k {
                    flush(&top, &mut local, &threshold, top_k);
                }
            }
        }
        flush(&top, &mut local, &threshold, top_k);
        c_full.fetch_add(lf, Ordering::Relaxed);
        c_early.fetch_add(le, Ordering::Relaxed);
        c_mid.fetch_add(lm, Ordering::Relaxed);
    });
    let mut v = top.into_inner().unwrap();
    v.sort_by(|a, b| b.buckets.cmp(&a.buckets).then(a.f.cmp(&b.f)).then(a.s.cmp(&b.s)));
    v.truncate(top_k);
    (
        v,
        PairStats {
            evaluated_full: c_full.into_inner() as u64,
            pruned_early: c_early.into_inner() as u64,
            pruned_mid: c_mid.into_inner() as u64,
        },
    )
}

fn flush(top: &Mutex<Vec<PairScore>>, local: &mut Vec<PairScore>, thr: &AtomicUsize, k: usize) {
    if local.is_empty() {
        return;
    }
    let mut t = top.lock().unwrap();
    t.append(local);
    t.sort_by(|a, b| b.buckets.cmp(&a.buckets));
    t.truncate(k);
    if t.len() == k {
        // Only pairs strictly better than the K-th best can still enter.
        // (Ties at the K-th place may be dropped; reported top K is still a valid top K.)
        thr.store(t[k - 1].buckets, Ordering::Relaxed);
    }
}

/// Joint buckets for a specific pair: Vec of member lists.
pub fn joint_buckets(m: &Matrix, f: usize, s: usize, set: &[u16]) -> Vec<Vec<u16>> {
    let mut map: std::collections::HashMap<u16, Vec<u16>> = std::collections::HashMap::new();
    for &a in set {
        let k = m.get(f, a) as u16 * 243 + m.get(s, a) as u16;
        map.entry(k).or_default().push(a);
    }
    map.into_values().collect()
}
