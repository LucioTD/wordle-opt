//! Precomputed feedback matrix (guess x answer, flat u8) plus a small bitset type.

use crate::data::Data;
use crate::pattern::feedback;
use rayon::prelude::*;
use std::io::{Read, Write};

pub struct Matrix {
    pub ng: usize,
    pub na: usize,
    /// Row-major: data[g * na + a]
    pub data: Vec<u8>,
}

impl Matrix {
    #[inline(always)]
    pub fn row(&self, g: usize) -> &[u8] {
        &self.data[g * self.na..(g + 1) * self.na]
    }
    #[inline(always)]
    pub fn get(&self, g: usize, a: u16) -> u8 {
        // Bounds-checked on purpose: a bad index must panic with a message,
        // never read random memory. The cost is negligible.
        assert!(g < self.ng && (a as usize) < self.na, "matrix index out of range: guess {g}, answer {a}");
        self.data[g * self.na + a as usize]
    }

    pub fn build(d: &Data) -> Matrix {
        let na = d.na();
        let ng = d.guesses.len();
        let answers: Vec<_> = d.answers.iter().map(|&g| d.guesses[g]).collect();
        let mut data = vec![0u8; ng * na];
        data.par_chunks_mut(na).enumerate().for_each(|(g, row)| {
            let gw = &d.guesses[g];
            for (a, cell) in row.iter_mut().enumerate() {
                *cell = feedback(gw, &answers[a]);
            }
        });
        Matrix { ng, na, data }
    }

    /// Fingerprint of the word lists, so a stale cache is never reused.
    fn fingerprint(d: &Data) -> u64 {
        let mut h: u64 = 0xcbf29ce484222325;
        let mut eat = |b: u8| {
            h ^= b as u64;
            h = h.wrapping_mul(0x100000001b3);
        };
        for w in &d.guess_str {
            w.bytes().for_each(&mut eat);
        }
        eat(b'|');
        for &a in &d.answers {
            d.guess_str[a].bytes().for_each(&mut eat);
        }
        h
    }

    /// Load from cache or build and save.
    pub fn load_or_build(d: &Data, force: bool) -> Matrix {
        let path = d.dir.join("cache").join("matrix.bin");
        let fp = Self::fingerprint(d);
        if !force {
            if let Ok(mut f) = std::fs::File::open(&path) {
                let mut hdr = [0u8; 24];
                if f.read_exact(&mut hdr).is_ok() {
                    let fp2 = u64::from_le_bytes(hdr[0..8].try_into().unwrap());
                    let ng = u64::from_le_bytes(hdr[8..16].try_into().unwrap()) as usize;
                    let na = u64::from_le_bytes(hdr[16..24].try_into().unwrap()) as usize;
                    if fp2 == fp && ng == d.guesses.len() && na == d.na() {
                        let mut data = vec![0u8; ng * na];
                        if f.read_exact(&mut data).is_ok() {
                            return Matrix { ng, na, data };
                        }
                    }
                }
            }
        }
        let t = std::time::Instant::now();
        let m = Self::build(d);
        eprintln!("built matrix {}x{} in {:.2?}", m.ng, m.na, t.elapsed());
        std::fs::create_dir_all(path.parent().unwrap()).ok();
        let mut f = std::fs::File::create(&path).expect("cannot write cache");
        f.write_all(&fp.to_le_bytes()).unwrap();
        f.write_all(&(m.ng as u64).to_le_bytes()).unwrap();
        f.write_all(&(m.na as u64).to_le_bytes()).unwrap();
        f.write_all(&m.data).unwrap();
        m
    }
}

/// Fixed-capacity bitset over answer columns. Used as the memo key for
/// candidate sets and for fast filtering in `play`.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct BitSet(pub Box<[u64]>);

#[allow(dead_code)]
impl BitSet {
    pub fn new(n: usize) -> BitSet {
        BitSet(vec![0u64; n.div_ceil(64)].into_boxed_slice())
    }
    pub fn from_cols(n: usize, cols: &[u16]) -> BitSet {
        let mut b = BitSet::new(n);
        for &c in cols {
            b.insert(c);
        }
        b
    }
    #[inline]
    pub fn insert(&mut self, c: u16) {
        self.0[(c >> 6) as usize] |= 1u64 << (c & 63);
    }
    #[inline]
    pub fn contains(&self, c: u16) -> bool {
        self.0[(c >> 6) as usize] >> (c & 63) & 1 == 1
    }
    pub fn len(&self) -> usize {
        self.0.iter().map(|w| w.count_ones() as usize).sum()
    }
    pub fn to_cols(&self) -> Vec<u16> {
        let mut v = Vec::new();
        for (i, &w) in self.0.iter().enumerate() {
            let mut w = w;
            while w != 0 {
                let t = w.trailing_zeros();
                v.push((i * 64) as u16 + t as u16);
                w &= w - 1;
            }
        }
        v
    }
}
