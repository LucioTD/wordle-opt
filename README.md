# wordle-opt

Finds the best Wordle first and second guesses by computation, not folklore.

Rust, multithreaded with rayon. Checks all 110M guess pairs exhaustively and solves full game trees with branch and bound.

## Results

Against 2372 possible answers (NYT's curated list plus every word NYT has actually used), with 14855 allowed guesses:

| goal | opener | result |
|---|---|---|
| fewest guesses on average | `TARSE` | 3.4207 avg, 0 failures (beam search) |
| solve in 4 or fewer | `CARLE` | 99.11% |
| hard mode | `SALET` | 3.5177 avg, 0 failures |
| best fixed pair | `SOARE` + `CLINT` | 1113 buckets, 96.25% in 4 or fewer |

Full tables, other answer lists and a backtest against all 1932 daily games: [`RESULTS.md`](RESULTS.md). Data sources: [`data/README.md`](data/README.md).

## Quick start (Windows)

1. Install Rust: https://rustup.rs (default options).
2. In this folder: `powershell -ExecutionPolicy Bypass -File .\run_all.ps1`
3. Open `RESULTS.md` when it finishes. Rerun the same command to resume after an interruption.

Levels: `-Level Quick` (beam search only), `Full` (default, exact on the top openers), `Everything` (exact on the whole shortlist and every universe; long).
On CachyOS: `./run_all.sh full`.

## Commands

| command | does |
|---|---|
| `build-matrix` | 14855 x 2372 pattern matrix, cached in `data/cache/` |
| `rank-first --universe full --p3 0` | entropy, expected size, worst bucket, buckets, exact P(<=2), exact P(<=3) for every word |
| `rank-pairs --universe full` | exhaustive fixed pair search, provably safe pruning |
| `solve-tree --first salet --beam 0 --json t.json` | game tree (adaptive 2nd guess). `--beam 0` = exact, `--mode le4` = maximize P(<=4), `--hard` = hard mode |
| `shortlist --auto 10 --beam 30` | trees for many openers, resumable |
| `pair-trees --top 5 --beam 0` | best fixed pairs, then optimal play from guess 3 |
| `simulate --tree t.json --against history-seq` | replay a tree against any universe or the real daily history |
| `play` | interactive helper: type `salet ..y.g`, get the best next guess |
| `report` | builds `RESULTS.md` |

`--universe`: `full` (default) | `nyt2309` | `unused` | `historical`.

## Exactness

- Feedback rules: unit tests on duplicate-letter cases plus a 700k-pair cross-check against an independent reference implementation.
- Pair search: exhaustive over all 110M unordered pairs, pruning only with a proven upper bound.
- Tree search: `--beam 0` considers every allowed guess at every node. Branch and bound uses admissible lower bounds only. With `--beam K`, big nodes keep the top K guesses (labeled "bK" in results), small nodes are always exhaustive.
- Hard mode: `--hard` on any tree command. Guesses must keep the last row's greens in place and reuse its yellow letters (NYT client rule). `simulate` counts rule violations for hard trees.
