#!/usr/bin/env bash
# Linux / CachyOS version of run_all.ps1. Usage: ./run_all.sh [quick|full|everything]
set -euo pipefail
cd "$(dirname "$0")"
LEVEL="${1:-full}"
B=./target/release/wordle-opt
mkdir -p results
cargo build --release
cargo test --release
$B build-matrix
for u in full nyt2309 unused historical; do [ -f results/rank_first_$u.json ] || $B rank-first --universe $u --p3 0; done
for u in full unused historical; do [ -f results/pairs_$u.json ] || $B rank-pairs --universe $u --top 100; done
for mode in expected le4; do
  $B shortlist --universe full --auto 10 --beam 30 --mode $mode
  $B shortlist --universe full --from shortlist_full_${mode}_b30.json --top 12 --beam 200 --mode $mode
  [ "$LEVEL" = full ] && $B shortlist --universe full --from shortlist_full_${mode}_b200.json --top 5 --beam 0 --mode $mode
  [ "$LEVEL" = everything ] && $B shortlist --universe full --auto 10 --beam 0 --mode $mode
  if [ "$LEVEL" = quick ]; then src=shortlist_full_${mode}_b200.json; beam=200; else src=shortlist_full_${mode}_exact.json; beam=0; fi
  [ -f results/tree_full_$mode.json ] || $B solve-tree --universe full --first best:$src --beam $beam --mode $mode --json tree_full_$mode.json
done
pb=0; [ "$LEVEL" = quick ] && pb=200
$B pair-trees --universe full --top 5 --beam $pb --mode expected
$B shortlist --universe full --auto 6 --beam 30 --mode expected --hard
if [ "$LEVEL" = quick ]; then hsrc=shortlist_full_expected-hard_b30.json; hb=30; else
  $B shortlist --universe full --from shortlist_full_expected-hard_b30.json --top 8 --beam 100 --mode expected --hard
  $B shortlist --universe full --from shortlist_full_expected-hard_b100.json --top 3 --beam 0 --mode expected --hard
  hsrc=shortlist_full_expected-hard_exact.json; hb=0; fi
[ -f results/tree_full_expected-hard.json ] || $B solve-tree --universe full --first best:$hsrc --beam $hb --mode expected --hard --json tree_full_expected-hard.json
for u in unused historical; do
  $B shortlist --universe $u --auto 6 --beam 100 --mode expected
  if [ "$LEVEL" = quick ]; then src=shortlist_${u}_expected_b100.json; else
    $B shortlist --universe $u --from shortlist_${u}_expected_b100.json --top 3 --beam 0 --mode expected
    src=shortlist_${u}_expected_exact.json; fi
  [ -f results/tree_${u}_expected.json ] || $B solve-tree --universe $u --first best:$src --beam $pb --mode expected --json tree_${u}_expected.json
done
for t in tree_full_expected.json tree_full_le4.json tree_full_expected-hard.json; do
  for a in full nyt2309 unused historical history-seq; do $B simulate --tree $t --against $a; done
done
$B simulate --tree tree_unused_expected.json --against unused
$B simulate --tree tree_historical_expected.json --against historical
$B report
echo "All done. Open RESULTS.md"
