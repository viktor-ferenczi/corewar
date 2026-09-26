#!/usr/bin/env python3
"""Evaluate results/runs.csv and write the report as Markdown, HTML and ODT (via pandoc)."""

import csv
import io
import math
import re
import subprocess
import zipfile
from collections import defaultdict
from datetime import date
from pathlib import Path

HERE = Path(__file__).resolve().parent
RESULTS = HERE / "results"
RUNS_CSV = RESULTS / "runs.csv"
REPORT = RESULTS / "report"  # .md, .html, .odt

HTML_STYLE = """<style>
body { max-width: none; font-family: sans-serif; }
table { border-collapse: collapse; font-size: 12px; }
th, td { border: 1px solid #ccc; padding: 2px 5px; text-align: right; }
th:nth-child(2), td:nth-child(2) { text-align: left; }
thead th { background: #eee; position: sticky; top: 0; }
</style>
"""


def load_pairs(path: Path) -> dict[tuple[str, str], dict]:
    """Merge both start orders into totals per unordered pair, keyed (a, b) and (b, a).

    Pairs not played yet read as zero games, so a report can be made while the tournament runs.
    """
    pairs = defaultdict(lambda: {"games": 0, "wins": 0, "losses": 0, "draws": 0})
    with path.open() as f:
        for r in csv.DictReader(f):
            for me, other, won, lost in (
                (r["first"], r["second"], r["first_wins"], r["second_wins"]),
                (r["second"], r["first"], r["second_wins"], r["first_wins"]),
            ):
                p = pairs[me, other]
                p["games"] += int(r["games"])
                p["wins"] += int(won)
                p["losses"] += int(lost)
                p["draws"] += int(r["draws"])
    return pairs


def elo_ratings(programs: list[str], pairs: dict, iterations: int = 10000) -> dict[str, float]:
    """Maximum likelihood Bradley-Terry strengths on the Elo scale, mean 1500.

    A draw counts as half a win for both sides. One virtual draw per pair keeps
    the rating of a program that never scores anything finite.
    """
    strength = {p: 1.0 for p in programs}
    for _ in range(iterations):
        new = {}
        for i in programs:
            score = sum(pairs[i, j]["wins"] + 0.5 * pairs[i, j]["draws"] + 0.5 for j in programs if j != i)
            denom = sum((pairs[i, j]["games"] + 1) / (strength[i] + strength[j]) for j in programs if j != i)
            new[i] = score / denom
        scale = math.exp(sum(math.log(v) for v in new.values()) / len(new))
        new = {p: v / scale for p, v in new.items()}
        delta = max(abs(math.log(new[p] / strength[p])) for p in programs)
        strength = new
        if delta < 1e-12:
            break
    return {p: 1500 + 400 * math.log10(s) for p, s in strength.items()}


def standings(programs: list[str], pairs: dict) -> list[dict]:
    elo = elo_ratings(programs, pairs)
    rows = []
    for p in programs:
        totals = {k: sum(pairs[p, o][k] for o in programs if o != p) for k in ("games", "wins", "losses", "draws")}
        rows.append(
            {
                "program": p,
                **totals,
                "points": (3 * totals["wins"] + totals["draws"]) / totals["games"] * 100,
                "score": (totals["wins"] + 0.5 * totals["draws"]) / totals["games"] * 100,
                "elo": elo[p],
            }
        )
    rows.sort(key=lambda r: -r["elo"])
    return rows


def md_table(header: list[str], rows: list[list]) -> str:
    # Pandoc sizes the ODT/HTML columns relative to the dash counts in the separator line.
    cells = [header] + [[str(c) for c in row] for row in rows]
    widths = [max(3, *(len(row[n]) for row in cells)) for n in range(len(header))]
    separator = [":" + "-" * w if n == 1 else "-" * w + ":" for n, w in enumerate(widths)]
    return "\n".join("| " + " | ".join(row) + " |" for row in [cells[0], separator] + cells[1:])


def matrix(order: list[str], pairs: dict, cell) -> str:
    header = ["#", "Program"] + [str(n) for n in range(1, len(order) + 1)]
    rows = [
        [n, p] + ["-" if o == p else cell(pairs[p, o]) if pairs[p, o]["games"] else "" for o in order]
        for n, p in enumerate(order, 1)
    ]
    return md_table(header, rows)


def build_markdown(runs_path: Path) -> str:
    with runs_path.open() as f:
        runs = list(csv.DictReader(f))
    pairs = load_pairs(runs_path)
    programs = sorted({p for p, _ in pairs})
    per_pair = sorted({v["games"] for v in pairs.values()})
    table = standings(programs, pairs)
    order = [r["program"] for r in table]
    games = sum(int(r["games"]) for r in runs)
    first_wins = sum(int(r["first_wins"]) for r in runs)
    second_wins = sum(int(r["second_wins"]) for r in runs)
    draws = games - first_wins - second_wins
    max_steps = sorted({r["max_steps"] for r in runs})

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

    return f"""---
title: Round robin of the surviving 1993 CoreWar programs
subtitle: A reproduction, not the original competition results
---

Generated on {date.today().isoformat()} by `report.py` from [`runs.csv`](runs.csv).

> These are **not** the results of the First Hungarian Memory War (CoreWar) Championship of 1993.
> The entries of that competition did not all survive, Viktor's own entry is missing too, and some
> programs here were never entries (by their own comments MICE and CHANG are the winner and runner-up of the 1985 championship). This is a new
> round robin played in {date.today().year} between the programs found in the `Historical` folder, so the
> ranking says nothing about how the original competition ended.

## Setup

- Engine: Viktor's `MARS.COM` (CoreWar MARS V1.0 by GM, 1993) running in DOSBox in statistics mode
  (`/P`, `/V`). This is not `COREWAR.EXE`, the reference implementation by Kovács Tamás, one of the organizers.
- The rules follow the 1993 championship rules in `SZABALY.TXT`: 8000 cell arena, at most 64 processes
  per program, programs may execute each other's code, and a war is a draw after {", ".join(max_steps)} steps.
- {len(programs)} programs, {len(runs) // 2} pairs, {"/".join(map(str, per_pair))} games per pair
  (half of them with each program starting first), {games} games in total.
- Start order: over all games the program starting first won {first_wins}, the second one won
  {second_wins}, and {draws} games were draws.

## Scoring

Elo is a maximum likelihood Bradley-Terry rating on the Elo scale (400 points means 10:1 odds), where a
draw counts as half a win. It is fitted to all games at once, so unlike a running Elo it does not depend
on the order of the games. The average rating is 1500. Each pair gets one extra virtual draw, which keeps
the rating of a program that never scores finite.

Points use the usual CoreWar tournament scoring: 3 for a win and 1 for a draw, averaged per 100 games
(at most 300). Score % is wins plus half the draws as a percentage of the games played.

## Example battle

MICE (cyan) against KILLER (magenta), 14000 steps in. The bars at the bottom left show the number of
processes of each program, the counter on the right shows the game number and the steps in thousands.
Videos: [opening, slowed down](../media/MICE_vs_KILLER_opening.mp4) and
[full game at 286 speed](../media/MICE_vs_KILLER_full.mp4).

![MICE against KILLER in MARS](../media/MICE_vs_KILLER_opening_25.png){{ width=60% }}

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


def landscape_reference_odt(path: Path) -> None:
    """Pandoc's default ODT styles turned into A4 landscape with small table text, so the matrices fit."""
    data = subprocess.run(
        ["pandoc", "--print-default-data-file", "reference.odt"], capture_output=True, check=True
    ).stdout
    with zipfile.ZipFile(io.BytesIO(data)) as src, zipfile.ZipFile(path, "w") as dst:
        for item in src.infolist():
            content = src.read(item.filename)
            if item.filename == "styles.xml":
                xml = content.decode()
                xml = re.sub(r'fo:page-width="[^"]*"', 'fo:page-width="11.69in"', xml)
                xml = re.sub(r'fo:page-height="[^"]*"', 'fo:page-height="8.27in"', xml)
                xml = xml.replace('style:print-orientation="portrait"', 'style:print-orientation="landscape"')
                xml = re.sub(r'fo:margin-(top|bottom|left|right)="1in"', r'fo:margin-\1="0.4in"', xml)
                xml = re.sub(
                    r'(<style:style style:name="Table_20_Contents".*?)(</style:style>)',
                    r'\1<style:text-properties fo:font-size="6pt" />\2',
                    xml,
                    count=1,
                    flags=re.S,
                )
                content = xml.encode()
            dst.writestr(item, content)


def main() -> None:
    md = REPORT.with_suffix(".md")
    md.write_text(build_markdown(RUNS_CSV))
    header = RESULTS / "style.html"
    header.write_text(HTML_STYLE)
    reference = RESULTS / "reference.odt"
    landscape_reference_odt(reference)
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
