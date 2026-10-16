//! Game-tree search.
//!
//! cost(S, d) = sum over answers in S of the guesses used to solve them, counted
//! from guess number d onward, under the best policy.
//!
//! Objectives (one u64: late * W + total):
//!  * Mode::Expected  minimize total guesses (=> average), never exceed 6.
//!  * Mode::Le4       minimize #answers needing 5 or 6 guesses, then total guesses.
//!
//! Exactness:
//!  * beam = 0        EXACT. Every allowed guess is considered at every node.
//!  * beam = K > 0    nodes with |S| > exact_n only consider the top K guesses by
//!                    expected remaining size. This is the only heuristic cut.
//! Branch and bound only uses admissible lower bounds, so it never changes results.

use crate::matrix::{BitSet, Matrix};
use crate::pattern::{Word, ALL_GREEN, NPAT};
use rayon::prelude::*;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

pub const INF: u64 = u64::MAX / 4;
const W: u64 = 1 << 24;
pub const MAX_GUESSES: u8 = 6;
/// Nodes at least this big (at depth <= PAR_DEPTH) evaluate guesses in parallel.
const PAR_N: usize = 8;
/// Only fan out at guesses 2 and 3; deeper nodes are too small to be worth it.
const PAR_DEPTH: u8 = 3;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Expected,
    Le4,
}

#[derive(Clone)]
pub struct Config {
    pub mode: Mode,
    pub beam: usize,
    pub exact_n: usize,
    pub memo_cap: usize,
    pub col_guess: Arc<Vec<usize>>,
    /// Guesses forced at depth 1, 2, ... (fixed opener / fixed pair).
    pub forced: Vec<usize>,
    /// NYT hard mode: greens from the previous guess stay in place and its
    /// yellow letters must be reused (with multiplicity).
    pub hard: bool,
    pub words: Arc<Vec<Word>>,
}

/// Previous guess and its feedback (only needed in hard mode).
pub type Prev = Option<(u32, u8)>;

/// Hard-mode constraint left by the previous guess, packed into a u64:
/// green letter per position (5 bits each) + sorted yellow letters (5 bits each).
/// Different previous guesses that leave the same constraint share memo entries.
pub fn hard_sig(words: &[Word], prev: Prev) -> u64 {
    let Some((pg, pp)) = prev else { return u64::MAX };
    let pw = &words[pg as usize];
    let mut x = pp;
    let mut sig: u64 = 0;
    let mut yel: Vec<u8> = Vec::new();
    for i in 0..5 {
        let dgt = x % 3;
        x /= 3;
        if dgt == 2 {
            sig |= ((pw[i] + 1) as u64) << (5 * i);
        } else if dgt == 1 {
            yel.push(pw[i] + 1);
        }
    }
    yel.sort_unstable();
    for (j, &c) in yel.iter().enumerate() {
        sig |= (c as u64) << (25 + 5 * j);
    }
    sig
}

/// NYT hard-mode check, mirroring the game client: every green of the last
/// guess must be in the same position, and every yellow letter of the last
/// guess must appear among the remaining letters (each tile consumes one copy).
/// Hints from earlier rows persist automatically because each row had to obey
/// the row before it.
pub fn hard_ok(words: &[Word], g: usize, prev: Prev) -> bool {
    let Some((pg, pp)) = prev else { return true };
    let (gw, pw) = (&words[g], &words[pg as usize]);
    let mut digits = [0u8; 5];
    let mut x = pp;
    for d in digits.iter_mut() {
        *d = x % 3;
        x /= 3;
    }
    let mut pool = [0u8; 26];
    for i in 0..5 {
        if digits[i] == 2 {
            if gw[i] != pw[i] {
                return false;
            }
        } else {
            pool[gw[i] as usize] += 1;
        }
    }
    for i in 0..5 {
        if digits[i] == 1 {
            let c = pw[i] as usize;
            if pool[c] == 0 {
                return false;
            }
            pool[c] -= 1;
        }
    }
    true
}

type Key = (BitSet, u8, u64);

/// Memo shared by every thread working on one opener. Split into shards so
/// threads rarely wait on the same lock. Every stored value is exact (nodes are
/// always solved fully), so it does not matter which thread computed it.
pub struct Memo {
    shards: Vec<Mutex<HashMap<Key, (u64, u32)>>>,
    len: AtomicUsize,
    cap: usize,
}

impl Memo {
    pub fn new(cap: usize) -> Arc<Memo> {
        Arc::new(Memo { shards: (0..64).map(|_| Mutex::new(HashMap::new())).collect(), len: AtomicUsize::new(0), cap })
    }
    fn shard(&self, k: &Key) -> &Mutex<HashMap<Key, (u64, u32)>> {
        let mut h = k.1 as u64 ^ k.2.rotate_left(17);
        for w in k.0 .0.iter() {
            h = (h ^ w).wrapping_mul(0x9E3779B97F4A7C15);
        }
        &self.shards[(h >> 58) as usize]
    }
    fn get(&self, k: &Key) -> Option<(u64, u32)> {
        self.shard(k).lock().unwrap().get(k).copied()
    }
    fn insert(&self, k: Key, v: (u64, u32)) {
        if self.len.load(Ordering::Relaxed) >= self.cap {
            // Bounded RAM: flush everything. Entries are recomputed when needed.
            for sh in &self.shards {
                sh.lock().unwrap().clear();
            }
            self.len.store(0, Ordering::Relaxed);
        }
        if self.shard(&k).lock().unwrap().insert(k, v).is_none() {
            self.len.fetch_add(1, Ordering::Relaxed);
        }
    }
}

pub struct Solver<'a> {
    pub m: &'a Matrix,
    pub cfg: Config,
    memo: Arc<Memo>,
    pub nodes: u64,
    /// Allow parallel candidate evaluation inside this solver.
    pub parallel: bool,
}

pub fn decode(c: u64) -> (u64, u64) {
    (c / W, c % W)
}

impl<'a> Solver<'a> {
    pub fn new(m: &'a Matrix, cfg: Config) -> Self {
        let cap = cfg.memo_cap;
        Solver { m, cfg, memo: Memo::new(cap), nodes: 0, parallel: false }
    }

    fn spawn(&self) -> Solver<'a> {
        Solver { m: self.m, cfg: self.cfg.clone(), memo: Arc::clone(&self.memo), nodes: 0, parallel: self.parallel }
    }

    #[inline]
    fn late_w(&self) -> u64 {
        if self.cfg.mode == Mode::Le4 { W } else { 0 }
    }

    /// Cheap admissible lower bound for a set of size n first attacked at guess d.
    #[inline]
    fn lb(&self, n: usize, d: u8) -> u64 {
        if n == 0 {
            return 0;
        }
        if d > MAX_GUESSES || (n > 1 && d + 1 > MAX_GUESSES) || (n > 243 && d + 2 > MAX_GUESSES) {
            return INF;
        }
        let rest = n as u64 - 1;
        // One solved now, at most 242 one guess later, the rest at least 2 later.
        let total = 1 + 2 * rest.min(242) + 3 * rest.saturating_sub(242);
        let late = match d {
            0..=3 => 0,
            4 => rest,
            _ => n as u64,
        };
        total + late * self.late_w()
    }

    pub fn solve(&mut self, s: &[u16], d: u8, prev: Prev) -> u64 {
        self.solve_g(s, d, prev).0
    }

    /// Returns (cost, best guess). usize::MAX as guess means infeasible.
    pub fn solve_g(&mut self, s: &[u16], d: u8, prev: Prev) -> (u64, usize) {
        let n = s.len();
        let late_now = if d > 4 { self.late_w() } else { 0 };
        let forced = self.cfg.forced.get(d as usize - 1).copied();
        if d > MAX_GUESSES {
            return (INF, usize::MAX);
        }
        if forced.is_none() {
            if n == 1 {
                return (1 + late_now, self.cfg.col_guess[s[0] as usize]);
            }
            if n == 2 {
                if d + 1 > MAX_GUESSES {
                    return (INF, usize::MAX);
                }
                let late_next = if d + 1 > 4 { self.late_w() } else { 0 };
                return (3 + late_now * 2 + late_next, self.cfg.col_guess[s[0] as usize]);
            }
        }
        // Fast path: a candidate that splits S into singletons reaches the
        // cheap lower bound exactly (one solved now, all others next guess),
        // so it is optimal in both modes. Costs |S|^2 instead of a full scan.
        if forced.is_none() && n <= 243 {
            let lbn = self.lb(n, d);
            if lbn < INF {
                let mut seen = [0u32; NPAT];
                for (i, &c) in s.iter().enumerate() {
                    let g = self.cfg.col_guess[c as usize];
                    let row = self.m.row(g);
                    let stamp = i as u32 + 1;
                    let mut perfect = true;
                    for &a in s {
                        let p = row[a as usize] as usize;
                        if seen[p] == stamp {
                            perfect = false;
                            break;
                        }
                        seen[p] = stamp;
                    }
                    if perfect {
                        return (lbn, g);
                    }
                }
            }
        }
        let sig = if self.cfg.hard { hard_sig(&self.cfg.words, prev) } else { u64::MAX };
        let key = (BitSet::from_cols(self.m.na, s), d, sig);
        if let Some((c, g)) = self.memo.get(&key) {
            return (c, if g == u32::MAX { usize::MAX } else { g as usize });
        }
        // Guess 5 with 3+ candidates: the only way to finish by guess 6 is a guess
        // that splits the set into singletons. One scan decides it exactly, no
        // recursion and no beam needed. (Candidate perfect splits were handled above.)
        if forced.is_none() && d + 1 == MAX_GUESSES {
            let mut seen = [0u32; NPAT];
            let mut found = usize::MAX;
            for g in 0..self.m.ng {
                if self.cfg.hard && !hard_ok(&self.cfg.words, g, prev) {
                    continue;
                }
                let row = self.m.row(g);
                let stamp = g as u32 + 1;
                let mut perfect = true;
                for &a in s {
                    let p = row[a as usize] as usize;
                    if seen[p] == stamp {
                        perfect = false;
                        break;
                    }
                    seen[p] = stamp;
                }
                if perfect {
                    found = g;
                    break; // any non-candidate perfect split costs the same: 2n
                }
            }
            let res = if found == usize::MAX {
                (INF, usize::MAX)
            } else {
                let c = self.eval_guess(found, s, d, INF, false, false);
                (c, found)
            };
            let enc = if res.1 == usize::MAX { u32::MAX } else { res.1 as u32 };
            self.memo.insert(key, (res.0, enc));
            return res;
        }
        self.nodes += 1;

        let (order, tight) = match forced {
            Some(g) => (vec![g], 0),
            None => self.ranked_guesses(s, prev),
        };
        // Node lower bound: max of the cheap bound and the scan bound
        // 3n - max_g(k_g + 2[g in S]) (every child of size c costs >= 2c - 1).
        let node_lb = {
            let l = self.lb(n, d);
            if l >= INF { INF } else { l.max(tight + (l / W) * W) }
        };

        let (best, best_g) = self.search(&order, s, d, node_lb, forced.is_some());
        let (best, best_g) = if best >= INF && forced.is_none() && self.cfg.beam != 0 && n > self.cfg.exact_n {
            // The beam found no plan within 6 guesses. Never accept a failure
            // because of the heuristic cut: retry this node with every guess.
            self.solve_full(s, d, prev, node_lb)
        } else {
            (best, best_g)
        };
        let enc = if best_g == usize::MAX { u32::MAX } else { best_g as u32 };
        self.memo.insert(key, (best, enc));
        (best, best_g)
    }

    /// Evaluate candidate guesses in order and return the best (cost, guess).
    /// Big nodes near the top of the tree fan out over all threads: the long
    /// tail of a few hard buckets is where the time goes, so splitting only at
    /// the first level leaves most cores idle.
    fn search(&mut self, order: &[usize], s: &[u16], d: u8, node_lb: u64, forced: bool) -> (u64, usize) {
        let n = s.len();
        if self.parallel && d <= PAR_DEPTH && n >= PAR_N && order.len() > 1 {
            // Parallel candidate evaluation with a shared bound. Strict pruning
            // keeps ties alive, and the reduction prefers the earlier guess, so the
            // chosen guess is deterministic.
            let shared = AtomicU64::new(INF);
            let win = order
                .par_iter()
                .enumerate()
                .map(|(i, &g)| {
                    let bound = shared.load(Ordering::Relaxed);
                    if bound <= node_lb && bound < INF {
                        return (INF, i, g, None);
                    }
                    let mut sub = self.spawn();
                    let c = sub.eval_guess(g, s, d, bound, true, forced);

                    if c < INF {
                        shared.fetch_min(c, Ordering::Relaxed);
                    }
                    (c, i, g, Some(sub))
                })
                .reduce_with(|a, b| if (a.0, a.1) <= (b.0, b.1) { a } else { b })
                .unwrap();
            if let Some(sub) = win.3 {
                self.nodes += sub.nodes;
            }
            (win.0, if win.0 < INF { win.2 } else { usize::MAX })
        } else {
            let mut best = INF;
            let mut best_g = usize::MAX;
            for &g in order {
                if best <= node_lb {
                    break; // provably optimal
                }
                let c = self.eval_guess(g, s, d, best, false, forced);
                if c < best {
                    best = c;
                    best_g = g;
                }
            }
            (best, best_g)
        }
    }

    /// Search over every useful guess (ignores the beam).
    fn solve_full(&mut self, s: &[u16], d: u8, prev: Prev, node_lb: u64) -> (u64, usize) {
        let saved = self.cfg.beam;
        self.cfg.beam = 0;
        let (order, _) = self.ranked_guesses(s, prev);
        self.cfg.beam = saved;
        self.search(&order, s, d, node_lb, false)
    }

    /// Cost of playing g at (s, d), or INF if it cannot beat `bound`
    /// (strict: cannot be < bound; with `keep_ties`: cannot be <= bound).
    fn eval_guess(&mut self, g: usize, s: &[u16], d: u8, bound: u64, keep_ties: bool, forced: bool) -> u64 {
        let n = s.len();
        let late_now = if d > 4 { self.late_w() } else { 0 };
        let row = self.m.row(g);
        let mut idx = [u16::MAX; NPAT];
        let mut parts: Vec<(u8, Vec<u16>)> = Vec::new();
        let mut green = false;
        for &a in s {
            let p = row[a as usize];
            if p == ALL_GREEN {
                green = true;
                continue;
            }
            let i = idx[p as usize];
            if i == u16::MAX {
                idx[p as usize] = parts.len() as u16;
                parts.push((p, vec![a]));
            } else {
                parts[i as usize].1.push(a);
            }
        }
        if !green && parts.len() == 1 && !forced {
            return INF; // no information
        }
        let cut = |acc: u64| if keep_ties { acc > bound } else { acc >= bound };
        parts.sort_unstable_by_key(|v| std::cmp::Reverse(v.1.len()));
        let base = n as u64 + if green { late_now } else { 0 };
        let mut rem_lb: u64 = parts.iter().map(|v| self.lb(v.1.len(), d + 1)).fold(0, |a, b| a.saturating_add(b));
        let mut acc = base;
        if rem_lb >= INF || cut(acc.saturating_add(rem_lb)) {
            return INF;
        }
        for (p, v) in &parts {
            rem_lb -= self.lb(v.len(), d + 1);
            let c = self.solve(v, d + 1, Some((g as u32, *p)));
            acc = acc.saturating_add(c);
            if acc >= INF || cut(acc.saturating_add(rem_lb)) {
                return INF;
            }
        }
        acc
    }

    /// Guesses ordered by expected remaining size (sum of squared bucket sizes),
    /// candidates first on ties, plus the scan lower bound on total guesses.
    pub fn ranked_guesses(&self, s: &[u16], prev: Prev) -> (Vec<usize>, u64) {
        let n = s.len();
        let mut inset = vec![false; self.m.ng];
        for &a in s {
            inset[self.cfg.col_guess[a as usize]] = true;
        }
        let mut scored: Vec<(u64, usize)> = Vec::with_capacity(self.m.ng);
        let mut h = [0u32; NPAT];
        let mut best_split = 0u64; // max over g of (#non-green parts + 2[g in S])
        for g in 0..self.m.ng {
            let row = self.m.row(g);
            h.fill(0);
            for &a in s {
                h[row[a as usize] as usize] += 1;
            }
            let mut sq: u64 = 0;
            let mut nb = 0u64;
            for &c in h.iter() {
                if c > 0 {
                    sq += (c as u64) * (c as u64);
                    nb += 1;
                }
            }
            let gr = (h[ALL_GREEN as usize] > 0) as u64;
            if self.cfg.hard && !hard_ok(&self.cfg.words, g, prev) {
                continue; // illegal in hard mode (also excluded from the bound)
            }
            best_split = best_split.max(nb - gr + 2 * gr);
            if nb == 1 && gr == 0 {
                continue; // useless guess
            }
            scored.push((sq * 2 - gr, g));
        }
        scored.sort_unstable();
        let keep = if self.cfg.beam == 0 || n <= self.cfg.exact_n {
            scored.len()
        } else {
            self.cfg.beam.min(scored.len())
        };
        scored.truncate(keep);
        let tight = (3 * n as u64).saturating_sub(best_split);
        (scored.into_iter().map(|x| x.1).collect(), tight)
    }

    /// Guess number at which answer `a` is solved when playing the policy from (s, d).
    pub fn play_out(&mut self, s: &[u16], a: u16, mut d: u8, mut prev: Prev) -> u8 {
        let mut cur: Vec<u16> = s.to_vec();
        loop {
            let (_, g) = self.solve_g(&cur, d, prev);
            if g == usize::MAX {
                return 255;
            }
            let p = self.m.get(g, a);
            if p == ALL_GREEN {
                return d;
            }
            cur.retain(|&x| self.m.get(g, x) == p);
            prev = Some((g as u32, p));
            d += 1;
            if d > MAX_GUESSES {
                return 255;
            }
        }
    }
}

/// A solved opening: first guess plus one solver per first-guess feedback bucket.
/// Buckets are independent subproblems, so they are solved in parallel.
pub struct RootPlan<'a> {
    pub first: usize,
    pub cost: u64,
    pub nodes: u64,
    pub buckets: Vec<(u8, Vec<u16>, Solver<'a>)>,
}

impl<'a> RootPlan<'a> {
    pub fn solve(m: &'a Matrix, cfg: &Config, uni: &[u16], progress: Option<&str>) -> RootPlan<'a> {
        let first = cfg.forced[0];
        let mut groups: std::collections::BTreeMap<u8, Vec<u16>> = Default::default();
        for &a in uni {
            groups.entry(m.get(first, a)).or_default().push(a);
        }
        groups.remove(&ALL_GREEN);
        let mut items: Vec<(u8, Vec<u16>)> = groups.into_iter().collect();
        // Largest buckets first so the slowest work starts earliest.
        items.sort_by_key(|(_, v)| std::cmp::Reverse(v.len()));
        let total = items.len();
        let shared = Memo::new(cfg.memo_cap);
        let done = std::sync::atomic::AtomicUsize::new(0);
        let t0 = std::time::Instant::now();
        let solved: Vec<(u8, Vec<u16>, Solver<'a>, u64)> = items
            .into_par_iter()
            .map(|(p, v)| {
                let mut sv = Solver { m, cfg: cfg.clone(), memo: Arc::clone(&shared), nodes: 0, parallel: true };
                let c = sv.solve(&v, 2, Some((first as u32, p)));
                let k = done.fetch_add(1, Ordering::Relaxed) + 1;
                if let Some(label) = progress {
                    eprintln!("  [{label}] bucket {k}/{total} ({} answers) done at {:.1?}", v.len(), t0.elapsed());
                }
                (p, v, sv, c)
            })
            .collect();
        let mut cost = uni.len() as u64; // everyone pays guess 1; a green at guess 1 is never late
        let mut nodes = 0;
        let mut buckets = Vec::new();
        for (p, v, sv, c) in solved {
            cost = cost.saturating_add(c);
            nodes += sv.nodes;
            buckets.push((p, v, sv));
        }
        buckets.sort_by_key(|b| b.0);
        RootPlan { first, cost: cost.min(INF), nodes, buckets }
    }

    /// Depth at which answer a is solved (255 = failure).
    pub fn play_out(&mut self, m: &Matrix, a: u16) -> u8 {
        let p = m.get(self.first, a);
        if p == ALL_GREEN {
            return 1;
        }
        let b = self.buckets.iter_mut().find(|b| b.0 == p).expect("answer outside universe");
        let set = b.1.clone();
        b.2.play_out(&set, a, 2, Some((self.first as u32, p)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pattern::{feedback, parse_pattern, parse_word};

    fn ok(prev_guess: &str, colors: &str, guess: &str) -> bool {
        let words = vec![parse_word(prev_guess).unwrap(), parse_word(guess).unwrap()];
        hard_ok(&words, 1, Some((0, parse_pattern(colors).unwrap())))
    }

    #[test]
    fn hard_mode_rules() {
        // SALET -> L yellow, T green: T must stay at position 5, L must appear.
        assert!(ok("salet", "..y.g", "built"));
        assert!(!ok("salet", "..y.g", "crane")); // drops both hints
        assert!(!ok("salet", "..y.g", "lilts")); // T moved
        // Grays and known-wrong positions are still allowed (NYT behavior).
        assert!(ok("salet", "..y.g", "salet"));
        // First guess is unrestricted.
        let w = vec![parse_word("crane").unwrap()];
        assert!(hard_ok(&w, 0, None));
    }

    #[test]
    fn hard_mode_duplicates() {
        // EERIE vs EVENT = G Y . . . : E stays first AND a second E is required.
        let p = feedback(&parse_word("eerie").unwrap(), &parse_word("event").unwrap());
        let colors = crate::pattern::pattern_str(p).replace('G', "g").replace('Y', "y");
        assert_eq!(colors, "gy...");
        assert!(ok("eerie", &colors, "ember"));
        assert!(!ok("eerie", &colors, "eight")); // only the green E, yellow E missing
    }
}
