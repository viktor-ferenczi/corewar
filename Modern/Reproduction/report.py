#!/usr/bin/env python3
"""Evaluate results/runs.jsonl of the Rust engine tournament and write the report as Markdown, HTML
and ODT (via pandoc), with a comparison to the DOSBox tournament in Reproduction."""

import csv
import importlib.util
import json
import math
import subprocess
from collections import defaultdict
from datetime import date
from pathlib import Path

HERE = Path(__file__).resolve().parent
RESULTS = HERE / "results"
RUNS = RESULTS / "runs.jsonl"
REPORT = RESULTS / "report"  # .md, .html, .odt
DOSBOX = HERE.parent.parent / "Reproduction"
DOSBOX_CSV = DOSBOX / "results" / "runs.csv"
BASELINE = RESULTS / "baseline.jsonl"

# The helpers of the DOSBox report: pair totals, Elo, tables, ODT styles.
_spec = importlib.util.spec_from_file_location("dosbox_report", DOSBOX / "report.py")
dosbox_report = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(dosbox_report)
load_pairs, standings, md_table, matrix = (
    dosbox_report.load_pairs,
    dosbox_report.standings,
    dosbox_report.md_table,
    dosbox_report.matrix,
)


def read_runs(path: Path) -> list[dict]:
    """The runs of runs.jsonl, or of the DOSBox runs.csv, with numbers as numbers."""
    if path.suffix == ".jsonl":
        return [json.loads(line) for line in path.read_text().splitlines()]
    with path.open() as f:
        return [{k: v if k in ("first", "second") else float(v) for k, v in r.items()} for r in csv.DictReader(f)]


def jsonl_pairs(path: Path) -> dict[tuple[str, str], dict]:
    """Both start orders summed per pair, keyed (a, b) and (b, a), like load_pairs of the DOSBox report."""
    pairs = defaultdict(lambda: {"games": 0, "wins": 0, "losses": 0, "draws": 0})
    for r in read_runs(path):
        for me, other, won, lost in (
            (r["first"], r["second"], r["first_wins"], r["second_wins"]),
            (r["second"], r["first"], r["second_wins"], r["first_wins"]),
        ):
            p = pairs[me, other]
            p["games"] += r["games"]
            p["wins"] += won
            p["losses"] += lost
            p["draws"] += r["draws"]
    return pairs


def totals(path: Path) -> dict:
    runs = read_runs(path)
    games = int(sum(r["games"] for r in runs))
    first = int(sum(r["first_wins"] for r in runs))
    second = int(sum(r["second_wins"] for r in runs))
    # In the DOSBox CSV both start orders of a pair carry the time of the DOSBox session they ran in.
    seconds = sum(r.get("seconds", 0) for r in runs) / 2
    return {
        "runs": runs,
        "games": games,
        "first": first,
        "second": second,
        "draws": games - first - second,
        "seconds": seconds,
    }


def z_score(a: dict, b: dict) -> float:
    """Difference of the mean score per game (win 1, draw 1/2) between two samples in standard errors."""

    def moments(p: dict) -> tuple[float, float, int]:
        n = p["games"]
        mean = (p["wins"] + 0.5 * p["draws"]) / n
        return mean, (p["wins"] + 0.25 * p["draws"]) / n - mean**2, n

    (m1, v1, n1), (m2, v2, n2) = moments(a), moments(b)
    se = math.sqrt(v1 / n1 + v2 / n2)
    if se == 0:
        return 0.0 if m1 == m2 else math.inf
    return (m2 - m1) / se


def z_counts(programs: list[str], a: dict, b: dict) -> tuple[list, int, int]:
    pairs = [(x, y) for i, x in enumerate(programs) for y in programs[i + 1 :]]
    z = sorted(((abs(z_score(a[x, y], b[x, y])), x, y) for x, y in pairs), reverse=True)
    return z, sum(1 for v, *_ in z if v > 2), sum(1 for v, *_ in z if v > 3)


def comparison(programs: list[str], rust: dict, dosbox: dict, baseline: dict, rust_table: list[dict]) -> str:
    dosbox_table = standings(programs, dosbox)
    dosbox_rank = {r["program"]: (n, r) for n, r in enumerate(dosbox_table, 1)}
    rows = []
    for n, r in enumerate(rust_table, 1):
        m, d = dosbox_rank[r["program"]]
        rows.append([n, r["program"], f"{r['elo']:.0f}", m, f"{d['elo']:.0f}", f"{r['elo'] - d['elo']:+.0f}"])
    ranks = md_table(["Rank", "Program", "Elo", "DOSBox rank", "DOSBox Elo", "Difference"], rows)

    pairs = len(programs) * (len(programs) - 1) // 2
    z, over2, over3 = z_counts(programs, dosbox, rust)
    base_z, base2, base3 = z_counts(programs, baseline, rust)
    top = md_table(
        ["#", "Pair", "DOSBox score %", "Rust score %", "z"],
        [
            [
                n,
                f"{a} - {b}",
                f"{(dosbox[a, b]['wins'] + 0.5 * dosbox[a, b]['draws']) / dosbox[a, b]['games'] * 100:.1f}",
                f"{(rust[a, b]['wins'] + 0.5 * rust[a, b]['draws']) / rust[a, b]['games'] * 100:.1f}",
                f"{v:.2f}",
            ]
            for n, (v, a, b) in enumerate(z[:8], 1)
        ],
    )
    return f"""The two tournaments used different random seeds, so single results differ, but if the engines
behave the same, the differences are only random. For every pair the test below compares the mean
score per game (win 1, draw 1/2) of the 1000 games in each tournament, in standard errors (z).

Between DOSBox and this tournament {over2} of the {pairs} pairs are above |z| = 2 and {over3} above 3,
the largest |z| is {z[0][0]:.2f}. For independent games about {pairs * 0.0455:.0f} and {pairs * 0.0027:.1f}
would be expected. The games are not quite independent though: MARS places the programs with the next
outputs of a shift register, so the placements of consecutive wars are related, and the results of 500
wars in one run vary more than the test assumes. To see how much two tournaments differ by chance
alone, the Rust engine played the tournament once more with another master seed
([`baseline.jsonl`](baseline.jsonl)). Between the two Rust tournaments {base2} pairs are above 2 and
{base3} above 3, the largest |z| is {base_z[0][0]:.2f}, about the same as between DOSBox and Rust.

The pairs with the largest difference:

{top}

Ranking of both tournaments side by side:

{ranks}
"""


def build_markdown() -> str:
    rust_totals, dosbox_totals = totals(RUNS), totals(DOSBOX_CSV)
    runs = rust_totals["runs"]
    pairs = jsonl_pairs(RUNS)
    dosbox_pairs = load_pairs(DOSBOX_CSV)
    programs = sorted({p for p, _ in pairs})
    per_pair = sorted({v["games"] for v in pairs.values()})
    table = standings(programs, pairs)
    order = [r["program"] for r in table]
    max_steps = sorted({str(r["max_steps"]) for r in runs})
    timing = json.loads((RESULTS / "timing.json").read_text())
    seed = next(iter(timing.values()))["seed"]
    speed = []
    if "gpu" in timing:
        g = timing["gpu"]
        speed.append(f"- {len(g['devices'])} GPUs ({', '.join(g['devices'])}): {g['seconds']:.0f} seconds")
    if "cpu" in timing:
        c = timing["cpu"]
        speed.append(f"- {c['jobs']} CPU threads ({c['cpu']}): {c['seconds']:.0f} seconds")
    speed = "\n".join(speed)
    rustc = next(iter(timing.values()))["rustc"]

    ranking = md_table(
        ["Rank", "Program", "Elo", "Points", "Score %", "Wins", "Draws", "Losses", "Games"],
        [
            [
                n,
                r["program"],
                f"{r['elo']:.0f}",
                f"{r['points']:.1f}",
                f"{r['score']:.1f}",
                r["wins"],
                r["draws"],
                r["losses"],
                r["games"],
            ]
            for n, r in enumerate(table, 1)
        ],
    )
    score_matrix = matrix(order, pairs, lambda c: f"{(c['wins'] + 0.5 * c['draws']) / c['games'] * 100:.0f}")
    wins_matrix = matrix(order, pairs, lambda c: c["wins"])
    d = dosbox_totals

    return f"""---
title: Tournament of the surviving 1993 CoreWar programs on the native engine
subtitle: A reproduction, not the original competition results
---

Generated on {date.today().isoformat()} by `report.py` from [`runs.jsonl`](runs.jsonl).

> These are **not** the results of the First Hungarian Memory War (CoreWar) Championship of 1993.
> Some programs here were never entries (by their own comments MICE and CHANG are the winner and runner-up
> of the 1985 championship). This is the tournament of [`Reproduction`](../../../Reproduction) played
> again, between the programs found in the `Historical` folder, so the ranking says nothing about how
> the original competition ended.

## Setup

- Engine: the native Rust reimplementation of Viktor's `MARS.COM` in `Modern`, with the same
  compiler and simulator. Its test suite compares it with `MARS.COM` under DOSBox: the compiled code of
  every program and the exact statistics of 168 recorded battles with fixed random seeds are the same.
- The rules are the defaults of MARS, like in the DOSBox tournament: 8000 cell arena, at most 64
  processes per program, programs may execute each other's code, a war is a draw after
  {", ".join(max_steps)} steps, and a war without any DAT left in the arena ends as a draw.
- {len(programs)} programs, {len(runs) // 2} pairs, {"/".join(map(str, per_pair))} games per pair
  (half of them with each program starting first), {rust_totals['games']} games in total.
- Random seeds: MARS seeds its generator from the BIOS clock. Here every run gets a tick count derived
  from the master seed {seed}, stored in the `seed` field of `runs.jsonl`, so the tournament can be
  repeated exactly. Like in DOSBox, both start orders of a pair run one after the other in one
  emulated DOS session, where the second run inherits the memory after the arena from the first.
- Start order: over all games the program starting first won {rust_totals['first']}, the second one won
  {rust_totals['second']}, and {rust_totals['draws']} games were draws. In DOSBox these were {d['first']},
  {d['second']} and {d['draws']}.

## Speed

Wall clock time of the whole tournament, built with {rustc}:

{speed}

The GPUs and the CPU give exactly the same results.

The DOSBox tournament took less than 4 hours with 8 DOSBox instances in parallel, {d['seconds'] / 3600:.1f}
hours summed over the pairs.

## Comparison with the DOSBox tournament

{comparison(programs, pairs, dosbox_pairs, jsonl_pairs(BASELINE), table)}
## Scoring

Elo is a maximum likelihood Bradley-Terry rating on the Elo scale (400 points means 10:1 odds), where a
draw counts as half a win. It is fitted to all games at once, so unlike a running Elo it does not depend
on the order of the games. The average rating is 1500. Each pair gets one extra virtual draw, which keeps
the rating of a program that never scores finite.

Points use the usual CoreWar tournament scoring: 3 for a win and 1 for a draw, averaged per 100 games
(at most 300). Score % is wins plus half the draws as a percentage of the games played.

## Ranking

The ranking is by Elo.

{ranking}

## Score matrix

The score of the row program against the column program in percent (wins plus half the draws).
The column numbers are the ranks above.

{score_matrix}

## Win matrix

The number of games the row program won against the column program out of {"/".join(map(str, per_pair))}.
The losses are in the mirrored cell, the draws are the rest.

{wins_matrix}
"""


def main() -> None:
    md = REPORT.with_suffix(".md")
    md.write_text(build_markdown())
    header = RESULTS / "style.html"
    header.write_text(dosbox_report.HTML_STYLE)
    reference = RESULTS / "reference.odt"
    dosbox_report.landscape_reference_odt(reference)
    try:
        subprocess.run(["pandoc", "-s", "-H", str(header), str(md), "-o", str(REPORT.with_suffix(".html"))], check=True)
        subprocess.run(
            [
                "pandoc",
                f"--reference-doc={reference}",
                f"--resource-path={RESULTS}",
                str(md),
                "-o",
                str(REPORT.with_suffix(".odt")),
            ],
            check=True,
        )
    finally:
        header.unlink()
        reference.unlink()


if __name__ == "__main__":
    main()
