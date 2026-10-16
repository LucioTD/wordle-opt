//! wordle-opt: compute optimal Wordle first and second guesses.
//!
//! Subcommands: build-matrix, rank-first, rank-pairs, solve-tree, shortlist,
//! simulate, play. Run with --help for options.

mod data;
mod matrix;
mod metrics;
mod pairs;
mod pattern;
mod report;
mod tree;

use clap::{Parser, Subcommand};
use data::Data;
use matrix::Matrix;
use pattern::{pattern_str, ALL_GREEN};
use serde_json::{json, Value};
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::time::Instant;
use std::sync::Arc;
use tree::{decode, Config, Mode, RootPlan, Solver, INF};

#[derive(Parser)]
#[command(name = "wordle-opt", about = "Optimal Wordle opener search")]
struct Cli {
    /// Data directory (word lists, cache)
    #[arg(long, default_value = "data", global = true)]
    data: PathBuf,
    /// Output directory for JSON/markdown results
    #[arg(long, default_value = "results", global = true)]
    out: PathBuf,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(clap::Args, Clone)]
struct TreeArgs {
    /// expected | le4
    #[arg(long, default_value = "expected")]
    mode: String,
    /// Guesses considered at big nodes. 0 = EXACT (every allowed guess everywhere).
    #[arg(long, default_value_t = 30)]
    beam: usize,
    /// Nodes with at most this many candidates always consider every guess.
    #[arg(long, default_value_t = 12)]
    exact_n: usize,
    /// Max memo entries (shared by all threads) before a flush. 6M is about 2 to 3 GB.
    #[arg(long, default_value_t = 6_000_000)]
    memo_cap: usize,
    /// NYT hard mode (greens stay, yellows must be reused).
    #[arg(long)]
    hard: bool,
}

impl TreeArgs {
    fn opts(&self) -> TreeOpts {
        TreeOpts { mode: mode_of(&self.mode), beam: self.beam, exact_n: self.exact_n, memo_cap: self.memo_cap, hard: self.hard }
    }
    fn tag(&self) -> String {
        format!("{}{}_{}", self.mode, if self.hard { "-hard" } else { "" }, if self.beam == 0 { "exact".into() } else { format!("b{}", self.beam) })
    }
}

#[derive(Subcommand)]
enum Cmd {
    /// Build (or rebuild) the guess x answer pattern matrix cache.
    BuildMatrix {
        #[arg(long)]
        force: bool,
    },
    /// Score every allowed guess as a first word.
    RankFirst {
        #[arg(long, default_value = "full")]
        universe: String,
        #[arg(long, default_value_t = 20)]
        top: usize,
        /// Exact optimal P(solve <= 3) for this many top words by buckets. 0 = all words.
        #[arg(long, default_value_t = 150)]
        p3: usize,
    },
    /// Exhaustive fixed first+second pair search (provably safe pruning).
    RankPairs {
        #[arg(long, default_value = "full")]
        universe: String,
        #[arg(long, default_value_t = 100)]
        top: usize,
    },
    /// Solve the game tree for one opener (optionally a fixed second word).
    SolveTree {
        #[arg(long, default_value = "full")]
        universe: String,
        /// Opener, or "best:<shortlist json>" to take the winner of a shortlist run.
        #[arg(long)]
        first: String,
        /// Fixed second word, played after any non-green feedback.
        #[arg(long)]
        second: Option<String>,
        #[command(flatten)]
        t: TreeArgs,
        /// Write the decision tree to this JSON file (inside --out).
        #[arg(long)]
        json: Option<String>,
    },
    /// Solve trees for many openers and compare. Resumable.
    Shortlist {
        #[arg(long, default_value = "full")]
        universe: String,
        /// Comma separated openers.
        #[arg(long)]
        words: Option<String>,
        /// Use the union of the top K words by every cheap metric (needs rank-first).
        #[arg(long)]
        auto: Option<usize>,
        /// Take the best `top` openers from an earlier shortlist JSON (inside --out).
        #[arg(long)]
        from: Option<String>,
        #[arg(long, default_value_t = 5)]
        top: usize,
        #[command(flatten)]
        t: TreeArgs,
    },
    /// Trees for the best fixed pairs (needs rank-pairs). Resumable.
    PairTrees {
        #[arg(long, default_value = "full")]
        universe: String,
        #[arg(long, default_value_t = 5)]
        top: usize,
        #[command(flatten)]
        t: TreeArgs,
    },
    /// Evaluate a saved decision tree against an answer universe or the history.
    Simulate {
        /// Tree JSON (inside --out).
        #[arg(long)]
        tree: String,
        /// full | nyt2309 | unused | historical | history-seq
        #[arg(long, default_value = "full")]
        against: String,
    },
    /// Build RESULTS.md from everything in --out.
    Report,
    /// Interactive helper: enter your guess and colors, get the best next guess.
    Play {
        #[arg(long, default_value = "full")]
        universe: String,
        #[command(flatten)]
        t: TreeArgs,
    },
}

fn mode_of(s: &str) -> Mode {
    match s {
        "expected" => Mode::Expected,
        "le4" => Mode::Le4,
        _ => panic!("mode must be expected|le4"),
    }
}

fn col_guess(d: &Data) -> Vec<usize> {
    d.answers.clone()
}

fn guess_idx(d: &Data, w: &str) -> usize {
    d.find_guess(w).unwrap_or_else(|| panic!("'{w}' is not an allowed guess"))
}

fn dist_json(c: &[usize; 8]) -> Value {
    let n: usize = c.iter().sum();
    let tot: usize = (1..=6).map(|i| i * c[i]).sum();
    json!({
        "n": n, "solved_in": {"1": c[1], "2": c[2], "3": c[3], "4": c[4], "5": c[5], "6": c[6]},
        "fail": c[7], "avg": tot as f64 / n as f64,
        "p_le3": (c[1]+c[2]+c[3]) as f64 / n as f64,
        "p_le4": (c[1]+c[2]+c[3]+c[4]) as f64 / n as f64,
        "p_3or4": (c[3]+c[4]) as f64 / n as f64,
    })
}

fn dist_line(c: &[usize; 8]) -> String {
    let n: usize = c.iter().sum();
    let tot: usize = (1..=6).map(|i| i * c[i]).sum();
    format!(
        "n={n} | 1:{} 2:{} 3:{} 4:{} 5:{} 6:{} fail:{} | avg {:.4} | <=3 {:.2}% | <=4 {:.2}% | 3or4 {:.2}%",
        c[1], c[2], c[3], c[4], c[5], c[6], c[7],
        tot as f64 / n as f64,
        100.0 * (c[1] + c[2] + c[3]) as f64 / n as f64,
        100.0 * (c[1] + c[2] + c[3] + c[4]) as f64 / n as f64,
        100.0 * (c[3] + c[4]) as f64 / n as f64
    )
}

/// Export the decision tree rooted at (s, depth) as nested JSON.
fn export_node(sv: &mut Solver, dd: &Data, s: &[u16], depth: u8, prev: tree::Prev) -> Value {
    let (_, g) = sv.solve_g(s, depth, prev);
    if g == usize::MAX {
        return json!({"guess": null, "n": s.len(), "unsolved": true});
    }
    let mut children = serde_json::Map::new();
    let mut parts: std::collections::BTreeMap<u8, Vec<u16>> = Default::default();
    for &a in s {
        parts.entry(sv.m.get(g, a)).or_default().push(a);
    }
    for (p, v) in parts {
        if p == ALL_GREEN {
            continue;
        }
        children.insert(pattern_str(p), export_node(sv, dd, &v, depth + 1, Some((g as u32, p))));
    }
    let mut node = json!({"guess": dd.guess_str[g], "n": s.len()});
    if !children.is_empty() {
        node["children"] = Value::Object(children);
    }
    node
}

fn export_plan(plan: &mut RootPlan, dd: &Data, n: usize) -> Value {
    let mut children = serde_json::Map::new();
    let first = plan.first as u32;
    for (p, v, sv) in plan.buckets.iter_mut() {
        let set = v.clone();
        children.insert(pattern_str(*p), export_node(sv, dd, &set, 2, Some((first, *p))));
    }
    json!({"guess": dd.guess_str[plan.first], "n": n, "children": children})
}

fn write_out(out: &PathBuf, name: &str, v: &Value) {
    std::fs::create_dir_all(out).ok();
    let p = out.join(name);
    std::fs::write(&p, serde_json::to_string_pretty(v).unwrap()).unwrap();
    eprintln!("wrote {}", p.display());
}

fn read_json(out: &PathBuf, name: &str) -> Option<Value> {
    std::fs::read_to_string(out.join(name)).ok().and_then(|s| serde_json::from_str(&s).ok())
}

struct TreeOpts {
    hard: bool,
    mode: Mode,
    beam: usize,
    exact_n: usize,
    memo_cap: usize,
}

fn make_cfg(d: &Data, o: &TreeOpts, forced: Vec<usize>) -> Config {
    Config {
        mode: o.mode,
        beam: o.beam,
        exact_n: o.exact_n,
        memo_cap: o.memo_cap,
        col_guess: Arc::new(d.answers.clone()),
        forced,
        hard: o.hard,
        words: Arc::new(d.guesses.clone()),
    }
}

fn run_tree<'a>(
    d: &Data, m: &'a Matrix, uni: &[u16], first: usize, second: Option<usize>, o: &TreeOpts,
) -> (Value, RootPlan<'a>) {
    let mut forced = vec![first];
    forced.extend(second);
    let cfg = make_cfg(d, o, forced);
    let t = Instant::now();
    let label = d.guess_str[first].clone();
    let mut plan = RootPlan::solve(m, &cfg, uni, if o.beam == 0 { Some(label.as_str()) } else { None });
    let el = t.elapsed();
    let mut c = [0usize; 8];
    for &a in uni {
        let dd = plan.play_out(m, a) as usize;
        c[if (1..=6).contains(&dd) { dd } else { 7 }] += 1;
    }
    let (late, total) = decode(plan.cost);
    let res = json!({
        "first": d.guess_str[first],
        "second": second.map(|s| d.guess_str[s].clone()),
        "mode": format!("{:?}", o.mode), "hard": o.hard, "beam": o.beam, "exact": o.beam == 0, "exact_n": o.exact_n,
        "feasible": plan.cost < INF,
        "total_guesses": if o.mode == Mode::Expected { plan.cost } else { total },
        "late_answers": if o.mode == Mode::Le4 { Some(late) } else { None },
        "seconds": el.as_secs_f64(), "nodes": plan.nodes,
        "dist": dist_json(&c),
    });
    eprintln!(
        "{:>6} {:>6} {:?}{} beam={} -> {} ({:.1?}, {} nodes)",
        d.guess_str[first],
        second.map(|s| d.guess_str[s].as_str()).unwrap_or("-"),
        o.mode, if o.hard { " HARD" } else { "" }, if o.beam == 0 { "EXACT".to_string() } else { o.beam.to_string() }, dist_line(&c), el, plan.nodes
    );
    (res, plan)
}

/// Sort key for a tree result: lower is better.
fn tree_key(v: &Value) -> (u64, u64) {
    let late = v["late_answers"].as_u64().unwrap_or(0);
    let total = v["total_guesses"].as_u64().unwrap_or(u64::MAX);
    if v["feasible"].as_bool() == Some(false) { (u64::MAX, u64::MAX) } else if v["mode"] == "Le4" { (late, total) } else { (total, 0) }
}

/// Openers for a shortlist: union of the top k by every cheap metric.
fn auto_shortlist(out: &PathBuf, universe: &str, k: usize) -> Vec<String> {
    let v = read_json(out, &format!("rank_first_{universe}.json"))
        .expect("run rank-first for this universe first");
    let mut words: Vec<String> = Vec::new();
    for metric in ["entropy", "exp_size", "worst", "buckets", "p_le3"] {
        for s in v[metric].as_array().unwrap().iter().take(k) {
            let w = s["word"].as_str().unwrap().to_string();
            if !words.contains(&w) {
                words.push(w);
            }
        }
    }
    words
}

fn main() {
    let cli = Cli::parse();
    let d = Data::load(&cli.data);
    let t0 = Instant::now();
    let m = Matrix::load_or_build(&d, matches!(cli.cmd, Cmd::BuildMatrix { force: true }));
    eprintln!("matrix ready ({} x {}) in {:.2?}", m.ng, m.na, t0.elapsed());

    match cli.cmd {
        Cmd::BuildMatrix { .. } => {}
        Cmd::RankFirst { universe, top, p3 } => {
            let uni = d.universe(&universe);
            let t = Instant::now();
            let mut all = metrics::rank_all(&m, &uni, &d.guess_str);
            eprintln!("scored {} guesses in {:.2?}", all.len(), t.elapsed());
            // Exact optimal P(<=3) on the top `p3` words by bucket count.
            all.sort_by(|a, b| b.buckets.cmp(&a.buckets).then(b.entropy.total_cmp(&a.entropy)));
            let t = Instant::now();
            let cg = col_guess(&d);
            {
                use rayon::prelude::*;
                let n = if p3 == 0 { all.len() } else { p3.min(all.len()) };
                let vals: Vec<f64> = all[..n].par_iter().map(|s| metrics::p_le3(&m, s.guess, &uni, &cg)).collect();
                for (i, v) in vals.into_iter().enumerate() {
                    all[i].p_le3 = Some(v);
                }
            }
            eprintln!("p_le3 for top {p3} in {:.2?}", t.elapsed());
            let mut out = serde_json::Map::new();
            let mut tables = String::new();
            type Cmp = Box<dyn Fn(&metrics::Stats, &metrics::Stats) -> std::cmp::Ordering>;
            let sorts: Vec<(&str, Cmp)> = vec![
                ("entropy", Box::new(|a, b| b.entropy.total_cmp(&a.entropy))),
                ("exp_size", Box::new(|a, b| a.exp_size.total_cmp(&b.exp_size))),
                ("worst", Box::new(|a, b| a.worst.cmp(&b.worst).then(a.exp_size.total_cmp(&b.exp_size)))),
                ("buckets", Box::new(|a, b| b.buckets.cmp(&a.buckets).then(b.entropy.total_cmp(&a.entropy)))),
                ("p_le3", Box::new(|a, b| {
                    b.p_le3.unwrap_or(-1.0).total_cmp(&a.p_le3.unwrap_or(-1.0)).then(b.entropy.total_cmp(&a.entropy))
                })),
            ];
            for (name, f) in sorts {
                let mut v = all.clone();
                v.sort_by(|a, b| f(a, b));
                v.truncate(top);
                tables.push_str(&format!("\n### Top {top} by {name} (universe: {universe}, N={})\n\n", uni.len()));
                tables.push_str("| # | word | entropy | exp size | worst | buckets | P(<=2) | P(<=3) | answer? |\n|---|---|---|---|---|---|---|---|---|\n");
                for (i, s) in v.iter().enumerate() {
                    tables.push_str(&format!(
                        "| {} | {} | {:.4} | {:.2} | {} | {} | {:.2}% | {} | {} |\n",
                        i + 1, s.word, s.entropy, s.exp_size, s.worst, s.buckets, 100.0 * s.p_le2,
                        s.p_le3.map(|x| format!("{:.2}%", 100.0 * x)).unwrap_or("-".into()),
                        if s.is_candidate { "yes" } else { "no" }
                    ));
                }
                out.insert(name.into(), serde_json::to_value(&v).unwrap());
            }
            // Full ranking (all words) for correlation checks.
            out.insert("all".into(), serde_json::to_value(&all).unwrap());
            println!("{tables}");
            write_out(&cli.out, &format!("rank_first_{universe}.json"), &Value::Object(out));
            std::fs::write(cli.out.join(format!("rank_first_{universe}.md")), tables).unwrap();
        }
        Cmd::RankPairs { universe, top } => {
            let uni = d.universe(&universe);
            let t = Instant::now();
            let (pairs, st) = pairs::search(&m, &uni, top);
            let el = t.elapsed();
            let total_pairs = (m.ng as u64) * (m.ng as u64 - 1) / 2;
            eprintln!(
                "pair search {:.2?}: {} unordered pairs, fully evaluated {}, pruned by whole-pair bound {}, pruned mid-scan {}",
                el, total_pairs, st.evaluated_full, st.pruned_early, st.pruned_mid
            );
            let n = uni.len() as f64;
            let cg = col_guess(&d);
            let mut rows = Vec::new();
            let mut md = format!(
                "\n### Top {} fixed pairs (universe: {universe}, N={})\n\nsearch: {:.1?}, {} pairs, {} fully scored, {} cut by bound, {} cut mid-scan\n\n| # | first | second | joint buckets | P(<=3) | P(<=4)* | worst joint | exp size |\n|---|---|---|---|---|---|---|---|\n",
                top, uni.len(), el, total_pairs, st.evaluated_full, st.pruned_early, st.pruned_mid
            );
            for (i, p) in pairs.iter().enumerate() {
                // Order the pair: the word with more buckets alone goes first.
                let (bf, bs) = (
                    metrics::stats_for(&m, p.f, &uni, "").buckets,
                    metrics::stats_for(&m, p.s, &uni, "").buckets,
                );
                let (f, s) = if bf >= bs { (p.f, p.s) } else { (p.s, p.f) };
                let jb = pairs::joint_buckets(&m, f, s, &uni);
                let worst = jb.iter().map(|b| b.len()).max().unwrap();
                let exp: f64 = jb.iter().map(|b| (b.len() * b.len()) as f64).sum::<f64>() / n;
                // P(<=4) with an optimal adaptive third guess (exact, only for the top 20).
                let p4 = if i < 20 {
                    let solved: usize =
                        jb.iter().map(|b| if b.len() == 1 { 1 } else { metrics::best_two_more(&m, b, &cg) }).sum();
                    Some(solved as f64 / n)
                } else {
                    None
                };
                md.push_str(&format!(
                    "| {} | {} | {} | {} | {:.2}% | {} | {} | {:.3} |\n",
                    i + 1, d.guess_str[f], d.guess_str[s], p.buckets, 100.0 * p.buckets as f64 / n,
                    p4.map(|x| format!("{:.2}%", 100.0 * x)).unwrap_or("-".into()), worst, exp
                ));
                rows.push(json!({"first": d.guess_str[f], "second": d.guess_str[s], "buckets": p.buckets,
                    "p_le3": p.buckets as f64 / n, "p_le4_adaptive_third": p4, "worst": worst, "exp_size": exp}));
            }
            md.push_str("\n*P(<=4): fixed pair, then the best possible adaptive third guess per joint bucket.\n");
            println!("{md}");
            write_out(&cli.out, &format!("pairs_{universe}.json"), &json!({"seconds": el.as_secs_f64(),
                "pairs_total": total_pairs, "evaluated_full": st.evaluated_full, "pruned_bound": st.pruned_early,
                "pruned_mid": st.pruned_mid, "top": rows}));
            std::fs::write(cli.out.join(format!("pairs_{universe}.md")), md).unwrap();
        }
        Cmd::SolveTree { universe, first, second, t, json: jname } => {
            let uni = d.universe(&universe);
            let first = if let Some(f) = first.strip_prefix("best:") {
                let v = read_json(&cli.out, f).expect("shortlist file not found");
                let mut rows: Vec<Value> = v.as_array().unwrap().clone();
                rows.sort_by_key(tree_key);
                rows[0]["first"].as_str().unwrap().to_string()
            } else {
                first
            };
            let f = guess_idx(&d, &first);
            let s2 = second.as_deref().map(|w| guess_idx(&d, w));
            let (mut res, mut plan) = run_tree(&d, &m, &uni, f, s2, &t.opts());
            // The adaptive layer: best second guess for each first-guess pattern.
            let mut seconds = serde_json::Map::new();
            for (p, v, sv) in plan.buckets.iter_mut() {
                let set = v.clone();
                let (_, g) = sv.solve_g(&set, 2, Some((f as u32, *p)));
                let name = if g == usize::MAX { "-".to_string() } else { d.guess_str[g].clone() };
                seconds.insert(pattern_str(*p), json!({"n": v.len(), "second": name}));
            }
            res["second_by_pattern"] = Value::Object(seconds);
            res["universe"] = json!(universe);
            println!("{}", serde_json::to_string_pretty(&res["dist"]).unwrap());
            if let Some(name) = jname {
                let tr = export_plan(&mut plan, &d, uni.len());
                write_out(&cli.out, &name, &json!({"universe": universe, "summary": res, "tree": tr}));
            }
        }
        Cmd::Shortlist { universe, words, auto, from, top, t } => {
            let uni = d.universe(&universe);
            let list: Vec<String> = if let Some(w) = words {
                w.split(',').map(|x| x.trim().to_string()).collect()
            } else if let Some(k) = auto {
                auto_shortlist(&cli.out, &universe, k)
            } else if let Some(f) = from {
                let v = read_json(&cli.out, &f).expect("--from file not found");
                let mut rows: Vec<Value> = v.as_array().unwrap().clone();
                rows.sort_by_key(tree_key);
                rows.iter().take(top).map(|r| r["first"].as_str().unwrap().to_string()).collect()
            } else {
                panic!("give --words, --auto or --from");
            };
            let name = format!("shortlist_{universe}_{}.json", t.tag());
            // Resume: keep finished openers from an earlier interrupted run.
            let mut rows: Vec<Value> = read_json(&cli.out, &name).and_then(|v| v.as_array().cloned()).unwrap_or_default();
            eprintln!("shortlist ({}): {}", list.len(), list.join(","));
            for (k, w) in list.iter().enumerate() {
                eprintln!("[{}/{}] {w} ...", k + 1, list.len());
                if rows.iter().any(|r| r["first"] == w.as_str()) {
                    eprintln!("{w}: already done, skipping");
                    continue;
                }
                let f = guess_idx(&d, w);
                let (res, _) = run_tree(&d, &m, &uni, f, None, &t.opts());
                rows.push(res);
                write_out(&cli.out, &name, &json!(rows));
            }
            rows.sort_by_key(tree_key);
            for r in &rows {
                println!("{:>6}  total {:>6}  late {:>4}  avg {:.4}  3or4 {:.2}%", r["first"].as_str().unwrap(),
                    r["total_guesses"], r["late_answers"].as_u64().unwrap_or(0),
                    r["dist"]["avg"].as_f64().unwrap(), 100.0 * r["dist"]["p_3or4"].as_f64().unwrap());
            }
        }
        Cmd::PairTrees { universe, top, t } => {
            let uni = d.universe(&universe);
            let v = read_json(&cli.out, &format!("pairs_{universe}.json")).expect("run rank-pairs first");
            let name = format!("pairtrees_{universe}_{}.json", t.tag());
            let mut rows: Vec<Value> = read_json(&cli.out, &name).and_then(|v| v.as_array().cloned()).unwrap_or_default();
            for p in v["top"].as_array().unwrap().iter().take(top) {
                let (f, s2) = (p["first"].as_str().unwrap(), p["second"].as_str().unwrap());
                if rows.iter().any(|r| r["first"] == f && r["second"] == s2) {
                    continue;
                }
                let (res, _) = run_tree(&d, &m, &uni, guess_idx(&d, f), Some(guess_idx(&d, s2)), &t.opts());
                rows.push(res);
                write_out(&cli.out, &name, &json!(rows));
            }
        }
        Cmd::Simulate { tree, against } => {
            let v: Value = read_json(&cli.out, &tree).expect("tree file not found");
            let hard = v["summary"]["hard"].as_bool().unwrap_or(false);
            let mut illegal = 0usize;
            let targets: Vec<u16> =
                if against == "history-seq" { d.history.iter().map(|x| x.1).collect() } else { d.universe(&against) };
            let mut c = [0usize; 8];
            let mut misses = Vec::new();
            for &a in &targets {
                let aw = d.answers[a as usize];
                let mut node = &v["tree"];
                let mut depth = 1usize;
                let mut prev: tree::Prev = None;
                loop {
                    let Some(gw) = node["guess"].as_str() else {
                        c[7] += 1;
                        misses.push(d.col_word(a).to_string());
                        break;
                    };
                    let g = guess_idx(&d, gw);
                    if hard && !tree::hard_ok(&d.guesses, g, prev) {
                        illegal += 1;
                    }
                    prev = Some((g as u32, m.get(g, a)));
                    if g == aw {
                        c[depth] += 1;
                        break;
                    }
                    let p = pattern_str(m.get(g, a));
                    match node.get("children").and_then(|ch| ch.get(&p)) {
                        Some(n2) if depth < 6 => {
                            node = n2;
                            depth += 1;
                        }
                        _ => {
                            c[7] += 1;
                            misses.push(d.col_word(a).to_string());
                            break;
                        }
                    }
                }
            }
            println!("{tree} vs {against}: {}{}", dist_line(&c),
                if hard { format!(" | hard-mode violations: {illegal}") } else { String::new() });
            let stem = tree.trim_end_matches(".json");
            write_out(&cli.out, &format!("sim_{stem}_vs_{against}.json"),
                &json!({"tree": tree, "against": against, "hard": hard, "hard_violations": illegal, "dist": dist_json(&c), "misses": misses}));
        }
        Cmd::Report => {
            let md = report::build(&cli.out, &d);
            std::fs::write("RESULTS.md", &md).unwrap();
            eprintln!("wrote RESULTS.md ({} bytes)", md.len());
        }
        Cmd::Play { universe, t } => {
            let uni = d.universe(&universe);
            let cfg = make_cfg(&d, &t.opts(), vec![]);
            let mut sv = Solver::new(&m, cfg);
            sv.parallel = true;
            let mut cur = uni.clone();
            let mut depth = 1u8;
            let mut prev: tree::Prev = None;
            if t.hard {
                println!("HARD MODE: suggestions respect the hard-mode rule.");
            }
            // Suggest the opener from the best saved tree if there is one.
            let opener = read_json(&cli.out, &format!("tree_{universe}_expected.json"))
                .and_then(|v| v["tree"]["guess"].as_str().map(|s| s.to_uppercase()));
            println!("Wordle helper ({} answers). Type: <guess> <colors> using g / y / . (example: salet ..y.g)", uni.len());
            println!("Commands: new, q.  Recommended opener: {}", opener.unwrap_or("run solve-tree first".into()));
            let stdin = std::io::stdin();
            loop {
                print!("[{} candidates, guess {}] > ", cur.len(), depth);
                std::io::stdout().flush().ok();
                let mut line = String::new();
                if stdin.lock().read_line(&mut line).unwrap() == 0 {
                    break;
                }
                let parts: Vec<&str> = line.split_whitespace().collect();
                match parts.as_slice() {
                    ["q"] | ["quit"] => break,
                    ["new"] => {
                        cur = uni.clone();
                        depth = 1;
                        prev = None;
                    }
                    [g, p] => {
                        let (Some(gi), Some(pat)) = (d.find_guess(g), pattern::parse_pattern(p)) else {
                            println!("bad input (word not allowed, or colors are not 5 of g/y/.)");
                            continue;
                        };
                        if t.hard && !tree::hard_ok(&d.guesses, gi, prev) {
                            println!("note: that guess breaks hard mode rules (the game would reject it)");
                        }
                        if pat == ALL_GREEN {
                            println!("Solved in {depth}!");
                            cur = uni.clone();
                            depth = 1;
                            prev = None;
                            continue;
                        }
                        prev = Some((gi as u32, pat));
                        cur.retain(|&a| m.get(gi, a) == pat);
                        depth += 1;
                        if cur.is_empty() {
                            println!("No known answer matches. Check the colors (or NYT used a brand new word).");
                            cur = uni.clone();
                            depth = 1;
                            prev = None;
                            continue;
                        }
                        let show: Vec<&str> = cur.iter().take(30).map(|&a| d.col_word(a)).collect();
                        println!("remaining {}: {}{}", cur.len(), show.join(" "), if cur.len() > 30 { " ..." } else { "" });
                        let t0 = Instant::now();
                        let (c, best) = sv.solve_g(&cur, depth, prev);
                        if best == usize::MAX || c >= INF {
                            println!("no plan finishes within 6 from here; guess any remaining word");
                            continue;
                        }
                        println!("best next guess: {}  (avg {:.3} guesses left incl. this one, {:.1?})",
                            d.guess_str[best].to_uppercase(), c as f64 / cur.len() as f64, t0.elapsed());
                    }
                    _ => println!("usage: <guess> <colors>"),
                }
            }
        }
    }
}
