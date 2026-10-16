# Data

Fetched 2026-10-02 (UTC). History runs through Wordle #1931 (2026-10-02).

| file | words | what |
|---|---|---|
| `allowed.txt` | 14855 | Every word Wordle accepts as a guess (includes all answers) |
| `answers_nyt2309.txt` | 2309 | NYT's curated answer list as shipped in the client before answers moved server side (Nov 2022) |
| `answers_full.txt` | 2372 | `nyt2309` plus every answer NYT has actually used (63 used words were never on the curated list) |
| `answers_historical.txt` | 1907 | Unique answers already played |
| `answers_unused.txt` | 465 | `full` minus already played |
| `history.csv` | 1932 rows | Every daily answer by number and date, with how many sources agree |

## Sources

- **Allowed guesses:** `tabatkins/wordle-list` (`words`, 14855). It equals the original 12972 accepted words plus the 1883 NYT added in 2022/2023. Every historical answer is in it. NYT does not publish the list, so additions after March 2023, if any, are not covered.
- **Curated answer list:** `stuartpb/wordles` (`wordles.json`, 2309, extracted from the NYT game JS). Cross-checked against `cfreshman`'s original 2315 list: the only difference is 6 words NYT removed (agora, fibre, lynch, pupal, slave, wench).
- **History (three independent sources, majority vote):**
  - wordlegroup.com/boards (all 1932 games, 40 pages)
  - puzznest.com/wordle/archive (1925 games, 7 missing)
  - stuckonwordle.com/wordle-answers-{2021..2026} (1931 games)
- The NYT daily API (`nytimes.com/svc/wordle/v2/DATE.json`) was blocked from the build sandbox. If you can reach it, it is the best tie breaker.

## Mismatches found (resolved 2 of 3)

| # | date | wordlegroup | puzznest | stuckonwordle | used |
|---|---|---|---|---|---|
| 241 | 2022-02-15 | agora | aroma | aroma | aroma (NYT edit) |
| 275 | 2022-03-21 | slosh | their | their | their (NYT reorder) |
| 276 | 2022-03-22 | their | slosh | slosh | slosh |
| 284 | 2022-03-30 | harry | stove | stove | stove (NYT edit) |
| 324 | 2022-05-09 | fetus | shine | shine | shine (NYT edit) |
| 325 | 2022-05-10 | butch | gecko | gecko | gecko |
| 777 | 2023-08-05 | anode | beget | anode | anode |
| 1058 | 2024-05-12 | outer | slice | outer | outer |
| 1069 | 2024-05-23 | swish | ebony | swish | swish |
| 1086 | 2024-06-09 | crowd | kappa | crowd | crowd |
| 1221 | 2024-10-22 | shout | stout | shout | shout |
| 1247 | 2024-11-17 | tally | cruel | tally | tally |
| 1875 | 2026-08-07 | feign | argot | feign | feign |
| 1891 | 2026-08-23 | olden | frump | olden | olden |
| 1892 | 2026-08-24 | runny | plonk | runny | runny |

Early 2022 rows are the known NYT edits after the acquisition (wordlegroup kept the pre-edit words). Later puzznest rows look like data errors on that site.

## Findings that shape the analysis

- **Repeats:** 25 answers have been reused since Feb 2026 (CIGAR on 2026-02-02 was first).
- **New words:** 63 answers were never on the curated list, and the share is rising (16 of the last 61 games in Aug/Sep 2026).
- So "unused" is not a good model of the future anymore. `full` is the primary target.

`raw/` keeps the parsed JSON and the word lists for audit. The scraped HTML pages are not committed.
