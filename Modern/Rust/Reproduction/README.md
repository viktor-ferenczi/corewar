# Reproduction on the native engine

The round robin of [`Reproduction`](../../../Reproduction) played again, this time on the Rust
reimplementation of `MARS.COM` in [`Modern/Rust`](..) instead of `MARS.COM` under DOSBox.

> This is **not** the original competition data of the First Hungarian Memory War (CoreWar)
> Championship of 1993. Some programs here were never entries at all. The results come from a
> tournament played in 2026 and do not tell how the original competition ended.

## Documents

- Results report: [Markdown](results/report.md), [HTML](results/report.html), [ODT](results/report.odt)
- Raw results, one row per pair and start order, with the random seed of each run: [results/runs.csv](results/runs.csv)
- Statistics output of every run, in the format `MARS.COM` prints, zipped: [raw.zip](raw.zip)
- The same tournament with another master seed, for the comparison in the report: [results/baseline.csv](results/baseline.csv)
- Run time and machine: [results/timing.json](results/timing.json)

## Results

630 pairs, 630000 games, 138 seconds on a Ryzen 7 9800X3D (8 cores, 16 threads). The DOSBox
tournament took almost 4 hours with 8 DOSBox instances in parallel.

The top five by Elo, see the [report](results/report.md) for the full ranking, the matrices and the
comparison with the DOSBox results:

| Rank | Program | Elo | Points | Wins | Draws | Losses |
|---:|:--|---:|---:|---:|---:|---:|
| 1 | PRB004 | 1677 | 195.6 | 17863 | 14877 | 2260 |
| 2 | PRB011 | 1662 | 189.1 | 16810 | 15750 | 2440 |
| 3 | MICE | 1654 | 183.5 | 15431 | 17916 | 1653 |
| 4 | PRB005 | 1648 | 185.3 | 16612 | 15005 | 3383 |
| 5 | MICE2 | 1634 | 172.8 | 13442 | 20171 | 1387 |

The ranking is the same as in DOSBox except for three swaps of neighbors further down, and every Elo rating
is within 4 points of the DOSBox one. The random seeds differ, so single pairs differ a bit, about as
much as two tournaments on the same engine with different seeds.

## How the tournament was run

Same programs in the same order as `mars.py`, 500 games per start order, the MARS defaults: 8000 cell
arena, 64 processes per program, programs may execute each other's code, a war is a draw after 600000
steps, and in statistics mode a war with no DAT left in the arena ends as a draw.

`MARS.COM` seeds its random generator from the BIOS clock. Here each run gets a BIOS tick count derived
from the master seed 1993, which is in the `seed` column of `runs.csv`. With the same seed, `mars run`
plays exactly the same games, and so does `MARS.COM` patched to that seed (see `tools/golden.py`).

`mars.py` played both start orders of a pair in one DOSBox, one after the other. `MARS.COM` never
clears the memory after the arena, where programs placed near its end spill over, so the second run
started with what the first one left there. The Rust tournament emulates this too.

Requirements: Rust (tested with 1.90), Python 3.12, and `pandoc` for the report.

Play the tournament, then the baseline with another seed, then make the report:

```bash
./tournament.py
```

```bash
./tournament.py --baseline 2
```

```bash
./report.py
```
