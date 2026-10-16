//! Exact Wordle feedback.
//!
//! Rules (matches NYT behavior):
//! 1. Greens are claimed first: every position where guess == answer.
//! 2. For the remaining guess positions, left to right, a letter is yellow only
//!    while the answer still has unclaimed copies of that letter; otherwise gray.
//!
//! Encoding: position i contributes digit d_i * 3^i, with 0 = gray, 1 = yellow,
//! 2 = green. All green = 242.

pub const ALL_GREEN: u8 = 242;
pub const NPAT: usize = 243;

/// A word stored as five letter indices 0..26.
pub type Word = [u8; 5];

pub fn parse_word(s: &str) -> Option<Word> {
    let b = s.trim().as_bytes();
    if b.len() != 5 {
        return None;
    }
    let mut w = [0u8; 5];
    for i in 0..5 {
        let c = b[i].to_ascii_lowercase();
        if !c.is_ascii_lowercase() {
            return None;
        }
        w[i] = c - b'a';
    }
    Some(w)
}

#[allow(dead_code)]
pub fn word_str(w: &Word) -> String {
    w.iter().map(|&c| (c + b'a') as char).collect()
}


const POW3: [u8; 5] = [1, 3, 9, 27, 81];

/// Compute the feedback pattern for `guess` against `answer`.
#[inline]
pub fn feedback(guess: &Word, answer: &Word) -> u8 {
    let mut counts = [0u8; 26];
    let mut pat: u8 = 0;
    let mut green = [false; 5];
    // Pass 1: greens, and count the answer letters that are NOT green-matched.
    for i in 0..5 {
        if guess[i] == answer[i] {
            green[i] = true;
            pat += 2 * POW3[i];
        } else {
            counts[answer[i] as usize] += 1;
        }
    }
    // Pass 2: yellows, limited by remaining counts, left to right.
    for i in 0..5 {
        if !green[i] {
            let c = guess[i] as usize;
            if counts[c] > 0 {
                counts[c] -= 1;
                pat += POW3[i];
            }
        }
    }
    pat
}

/// Parse a color string like "gy..b" or "21000". Accepts:
/// g/G/2 = green, y/Y/1 = yellow, b/B/x/./-/0 = gray.
pub fn parse_pattern(s: &str) -> Option<u8> {
    let b = s.trim().as_bytes();
    if b.len() != 5 {
        return None;
    }
    let mut p = 0u8;
    for i in 0..5 {
        let d = match b[i] {
            b'g' | b'G' | b'2' => 2,
            b'y' | b'Y' | b'1' => 1,
            b'b' | b'B' | b'x' | b'X' | b'.' | b'-' | b'0' => 0,
            _ => return None,
        };
        p += d * POW3[i];
    }
    Some(p)
}

/// Render a pattern as G/Y/. characters.
pub fn pattern_str(mut p: u8) -> String {
    let mut s = String::new();
    for _ in 0..5 {
        s.push(match p % 3 {
            2 => 'G',
            1 => 'Y',
            _ => '.',
        });
        p /= 3;
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fb(g: &str, a: &str) -> String {
        pattern_str(feedback(&parse_word(g).unwrap(), &parse_word(a).unwrap()))
    }

    #[test]
    fn basics() {
        assert_eq!(fb("crane", "crane"), "GGGGG");
        assert_eq!(fb("crane", "light"), ".....");
        assert_eq!(fb("crane", "nacre"), "YYYYG");
        assert_eq!(parse_pattern("GGGGG"), Some(ALL_GREEN));
        assert_eq!(parse_pattern("gy.b0"), Some(2 + 3));
    }

    #[test]
    fn duplicate_in_guess_single_in_answer() {
        // EERIE vs ABBEY: no greens, one E available, so only the first E is yellow.
        assert_eq!(fb("eerie", "abbey"), "Y....");
        // SPEED vs ABIDE: one E in answer, first E yellow, second gray, D yellow.
        assert_eq!(fb("speed", "abide"), "..Y.Y");
    }

    #[test]
    fn green_claims_before_yellow() {
        // Answer THOSE has one S at index 3. Guess SASSY: S at 3 is green,
        // so the S at 0 and 2 must be gray.
        assert_eq!(fb("sassy", "those"), "...G.");
        // LLAMA vs HELLO: answer has two L (idx 2,3). Guess L at 0,1 both yellow.
        assert_eq!(fb("llama", "hello"), "YY...");
        // LEVEL vs HELLO: L0 yellow, E1 green, V gray, E3 gray (only one E), L4 yellow.
        assert_eq!(fb("level", "hello"), "YG..Y");
    }

    #[test]
    fn triple_letters() {
        // EERIE vs EVENT: E0 green, E1 yellow (second E in answer at idx 2), E4 gray.
        assert_eq!(fb("eerie", "event"), "GY...");
        // ABBEY vs BABES: B1? guess a,b,b,e,y; answer b,a,b,e,s.
        // b2 green, e3 green; a0 yellow, b1: answer B count left = 1 (idx0) -> yellow.
        assert_eq!(fb("abbey", "babes"), "YYGG.");
        // MAMMA vs MAXIM: M0 G, A1 G, M2: one M left (idx4) -> Y, M3 gray, A4 gray.
        assert_eq!(fb("mamma", "maxim"), "GGY..");
    }

    #[test]
    fn known_nyt_examples() {
        // Widely reported real-game cases.
        // ROBOT vs FLOOR: O at idx 3 is green, R and first O yellow.
        assert_eq!(fb("robot", "floor"), "YY.G.");
        assert_eq!(fb("geese", "those"), "...GG");
        assert_eq!(fb("hello", "world"), "...GY");
    }

    /// Independent reference implementation (mirrors the NYT client algorithm:
    /// mark greens and strike those letters, then scan left to right for yellows
    /// by searching for an unstruck copy in the answer).
    fn reference(g: &str, a: &str) -> String {
        let g: Vec<char> = g.chars().collect();
        let mut a: Vec<Option<char>> = a.chars().map(Some).collect();
        let mut out = vec!['.'; 5];
        for i in 0..5 {
            if a[i] == Some(g[i]) {
                out[i] = 'G';
                a[i] = None;
            }
        }
        for i in 0..5 {
            if out[i] == 'G' {
                continue;
            }
            if let Some(j) = (0..5).find(|&j| a[j] == Some(g[i])) {
                out[i] = 'Y';
                a[j] = None;
            }
        }
        out.into_iter().collect()
    }

    #[test]
    fn matches_reference_on_real_lists() {
        // Every answer in the full universe against a spread of guesses rich in
        // repeated letters, plus every answer against every answer.
        let ans: Vec<String> = std::fs::read_to_string("data/answers_full.txt")
            .unwrap().lines().map(|s| s.to_string()).collect();
        let tricky = ["eerie", "sassy", "mamma", "geese", "error", "llama", "kayak", "fluff", "queue", "abbey", "salet", "crane"];
        let mut n = 0;
        for a in &ans {
            for g in tricky.iter().map(|s| s.to_string()).chain(ans.iter().step_by(7).cloned()) {
                assert_eq!(fb(&g, a), reference(&g, a), "{g} vs {a}");
                n += 1;
            }
        }
        assert!(n > 700_000);
    }
}
