//! Word lists and answer universes.
//!
//! Guess space: data/allowed.txt (every word Wordle accepts).
//! Matrix columns: data/answers_full.txt (the widest answer universe).
//! Every other universe is a subset of the full one, stored as column indices.

use crate::pattern::{parse_word, Word};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub struct Data {
    pub dir: PathBuf,
    /// All allowed guesses, alphabetical.
    pub guesses: Vec<Word>,
    pub guess_str: Vec<String>,
    pub guess_index: HashMap<String, usize>,
    /// Column j of the matrix is answer `answers[j]` (index into `guesses`).
    pub answers: Vec<usize>,
    /// Map from guess index to column index (if that word is in the full universe).
    pub answer_col: Vec<Option<u16>>,
    /// Historical sequence of answers (column indices), in date order, repeats kept.
    pub history: Vec<(String, u16)>,
}

fn read_list(p: &Path) -> Vec<String> {
    let s = std::fs::read_to_string(p).unwrap_or_else(|e| panic!("cannot read {}: {e}", p.display()));
    s.lines().map(|l| l.trim().to_lowercase()).filter(|l| l.len() == 5).collect()
}

impl Data {
    pub fn load(dir: &Path) -> Data {
        let guess_str = read_list(&dir.join("allowed.txt"));
        let guesses: Vec<Word> = guess_str.iter().map(|w| parse_word(w).expect("bad word")).collect();
        let guess_index: HashMap<String, usize> =
            guess_str.iter().enumerate().map(|(i, w)| (w.clone(), i)).collect();
        let full = read_list(&dir.join("answers_full.txt"));
        let answers: Vec<usize> = full
            .iter()
            .map(|w| *guess_index.get(w).unwrap_or_else(|| panic!("answer {w} not in allowed list")))
            .collect();
        let mut answer_col = vec![None; guesses.len()];
        for (j, &g) in answers.iter().enumerate() {
            answer_col[g] = Some(j as u16);
        }
        // history.csv: number,date,answer,sources,agree
        let mut history = Vec::new();
        if let Ok(s) = std::fs::read_to_string(dir.join("history.csv")) {
            for line in s.lines().skip(1) {
                let f: Vec<&str> = line.split(',').collect();
                if f.len() >= 3 {
                    let g = guess_index[f[2]];
                    history.push((f[1].to_string(), answer_col[g].expect("history answer not in full")));
                }
            }
        }
        Data { dir: dir.to_path_buf(), guesses, guess_str, guess_index, answers, answer_col, history }
    }

    pub fn na(&self) -> usize {
        self.answers.len()
    }

    /// Column indices of a named universe.
    ///  full       = NYT curated list (2309) + every historical answer
    ///  nyt2309    = NYT curated list as shipped before answers moved server side
    ///  unused     = full minus every answer already played
    ///  historical = unique answers already played
    pub fn universe(&self, name: &str) -> Vec<u16> {
        let file = match name {
            "full" => "answers_full.txt",
            "nyt2309" => "answers_nyt2309.txt",
            "unused" => "answers_unused.txt",
            "historical" => "answers_historical.txt",
            other => panic!("unknown universe {other} (use full|nyt2309|unused|historical)"),
        };
        let mut v: Vec<u16> = read_list(&self.dir.join(file))
            .iter()
            .map(|w| self.answer_col[self.guess_index[w]].unwrap())
            .collect();
        v.sort_unstable();
        v
    }

    pub fn col_word(&self, c: u16) -> &str {
        &self.guess_str[self.answers[c as usize]]
    }

    pub fn find_guess(&self, w: &str) -> Option<usize> {
        self.guess_index.get(&w.trim().to_lowercase()).copied()
    }
}
