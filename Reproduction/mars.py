#!/usr/bin/env python3
"""Run the historical 1993 MARS.COM (CoreWar MARS V1.0 by GM) under DOSBox.

watch       one battle with the VGA arena rendered in a DOSBox window
tournament  round robin of all programs, every pair in both start orders
"""

import argparse
import csv
import itertools
import os
import re
import subprocess
import sys
import tempfile
import time
from concurrent.futures import ThreadPoolExecutor, as_completed
from pathlib import Path

HERE = Path(__file__).resolve().parent
HISTORICAL = HERE.parent / "Historical"
RAW_DIR = HERE / "raw"
RUNS_CSV = HERE / "results" / "runs.csv"

# Meaningful programs from the Historical folder. Left out: NONE.CWR (idle loop),
# TEST.CWR (compiler test), ROHANO.CWR (copy of ROHAMO.CWR) and the extensionless
# files which are identical copies of their .CWR counterparts (except MICE2 and ARTUR-*).
PROGRAMS = [
    "ANTIIMP.CWR",
    "ARTUR-1",
    "ARTUR-2",
    "ARTUR-3",
    "CHANG.CWR",
    "CHANG2.CWR",
    "CREEPER.CWR",
    "GABOR1.CWR",
    "HARVKILL.CWR",
    "IMP.CWR",
    "KILLER.CWR",
    "KILLER01.CWR",
    "KILLER02.CWR",
    "KILLER03.CWR",
    "KILLER2.CWR",
    "MICE.CWR",
    "MICE2",
    "PRB001.CWR",
    "PRB002.CWR",
    "PRB003.CWR",
    "PRB004.CWR",
    "PRB005.CWR",
    "PRB006.CWR",
    "PRB007.CWR",
    "PRB008.CWR",
    "PRB009.CWR",
    "PRB010.CWR",
    "PRB011.CWR",
    "ROHAMO.CWR",
    "TORPE.CWR",
    "VIKTOR01.CWR",
    "VIKTOR02.CWR",
    "VIKTOR03.CWR",
    "VIKTOR04.CWR",
    "VIKTOR05.CWR",
    "Y.CWR",
]

RUN_FIELDS = [
    "first",
    "second",
    "games",
    "max_steps",
    "first_wins",
    "second_wins",
    "draws",
    "first_avg_pcs",
    "second_avg_pcs",
    "seconds",
]

STAT_LINE = re.compile(r"^\s*([12])\s+(\d+)\s+(\d+)\s+(\d+)\s+(\S+)\s*$")
WARS_LINE = re.compile(r"Number of full wars\s*=\s*(\d+)")


def name_of(program: str) -> str:
    return program.removesuffix(".CWR")


def dosbox_conf(workdir: Path, commands: list[str], cycles: str, window: str | None = None) -> Path:
    # A DOS command tail is limited to 127 characters, the short D:\ paths keep it well below that.
    video = f"[sdl]\noutput=openglnb\nwindowresolution={window}\n[render]\naspect=true\n" if window else ""
    conf = f"""{video}[dosbox]
machine=vgaonly
memsize=4
[cpu]
core=dynamic
cycles={cycles}
[mixer]
nosound=true
[speaker]
pcspeaker=false
tandy=off
[midi]
mpu401=none
[autoexec]
mount c "{workdir}"
mount d "{HISTORICAL}"
c:
{chr(10).join(commands)}
exit
"""
    path = workdir / "dosbox.conf"
    path.write_text(conf.replace("\n", "\r\n"))
    return path


def run_dosbox(conf: Path, headless: bool, timeout: float | None = None) -> None:
    env = dict(os.environ)
    if headless:
        env |= {"SDL_VIDEODRIVER": "dummy", "SDL_AUDIODRIVER": "dummy"}
    subprocess.run(
        ["dosbox", "-conf", str(conf)],
        env=env,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        timeout=timeout,
        check=True,
    )


def parse_sta(text: str) -> dict:
    """Parse the ASCII statistics MARS prints to stdout in /P mode."""
    wars = WARS_LINE.search(text)
    rows = {m[1]: m for m in map(STAT_LINE.match, text.replace("\r", "").splitlines()) if m}
    if not wars or set(rows) != {"1", "2"}:
        raise ValueError(f"Unexpected MARS output:\n{text}")
    games = int(wars[1])
    p1, p2 = rows["1"], rows["2"]
    return {
        "games": games,
        "first_wins": int(p1[3]),
        "second_wins": int(p2[3]),
        "draws": games - int(p1[3]) - int(p2[3]),
        "first_avg_pcs": int(p1[2]),
        "second_avg_pcs": int(p2[2]),
    }


def play_pair(a: str, b: str, games_per_order: int, max_steps: int, cycles: str) -> list[dict]:
    """Play a vs b and b vs a in one DOSBox session, return one row per start order."""
    with tempfile.TemporaryDirectory(prefix="mars-") as tmp:
        workdir = Path(tmp)
        opts = f"/P={games_per_order} /M={max_steps} /V"
        commands = [
            f"D:\\MARS D:\\{a} D:\\{b} {opts} /F=AB.LOG > AB.STA",
            f"D:\\MARS D:\\{b} D:\\{a} {opts} /F=BA.LOG > BA.STA",
        ]
        started = time.monotonic()
        run_dosbox(dosbox_conf(workdir, commands, cycles), headless=True, timeout=4 * 3600)
        seconds = round(time.monotonic() - started, 1)
        rows = []
        for first, second, sta in ((a, b, "AB.STA"), (b, a, "BA.STA")):
            text = (workdir / sta).read_text(encoding="cp852")
            (RAW_DIR / f"{name_of(first)}_vs_{name_of(second)}.sta").write_text(text)
            rows.append(
                {"first": name_of(first), "second": name_of(second), "max_steps": max_steps, "seconds": seconds}
                | parse_sta(text)
            )
        return rows


def tournament(args: argparse.Namespace) -> None:
    RAW_DIR.mkdir(exist_ok=True)
    RUNS_CSV.parent.mkdir(exist_ok=True)
    done = set()
    if RUNS_CSV.exists():
        with RUNS_CSV.open() as f:
            done = {(r["first"], r["second"]) for r in csv.DictReader(f)}
    else:
        with RUNS_CSV.open("w", newline="") as f:
            csv.DictWriter(f, RUN_FIELDS).writeheader()

    pairs = [(a, b) for a, b in itertools.combinations(PROGRAMS, 2) if (name_of(a), name_of(b)) not in done]
    print(f"{len(pairs)} pairs to play, {args.games} games each, {args.jobs} in parallel", flush=True)
    with ThreadPoolExecutor(args.jobs) as pool:
        futures = {pool.submit(play_pair, a, b, args.games // 2, args.max_steps, args.cycles): (a, b) for a, b in pairs}
        for n, future in enumerate(as_completed(futures), 1):
            rows = future.result()
            # Only the main thread appends, so the CSV needs no lock.
            with RUNS_CSV.open("a", newline="") as f:
                csv.DictWriter(f, RUN_FIELDS).writerows(rows)
            a, b = futures[future]
            print(f"[{n}/{len(pairs)}] {a} vs {b} in {rows[0]['seconds']} s", flush=True)


def watch(args: argparse.Namespace) -> None:
    for program in (args.first, args.second):
        if not (HISTORICAL / program).is_file():
            sys.exit(f"No such program in {HISTORICAL}: {program}")
    with tempfile.TemporaryDirectory(prefix="mars-") as tmp:
        command = f"D:\\MARS D:\\{args.first} D:\\{args.second} /S={args.delay} /M={args.max_steps}"
        run_dosbox(dosbox_conf(Path(tmp), [command], args.cycles, args.window), headless=False)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(required=True)

    p = sub.add_parser("watch", help="render one battle in a DOSBox window (ESC quits, any key closes at the end)")
    p.add_argument("first", help="program file name in Historical, e.g. MICE.CWR")
    p.add_argument("second", help="program file name in Historical, e.g. TORPE.CWR")
    p.add_argument("--delay", type=int, default=0, help="MARS /S slow-down loop count per step (default 0)")
    p.add_argument("--cycles", default="fixed 3000", help="DOSBox CPU cycles, 3000 is about a 286 (default)")
    p.add_argument("--max-steps", type=int, default=600000, help="MARS /M war length (default 600000)")
    p.add_argument("--window", default="1280x800", help="DOSBox window size (default 1280x800)")
    p.set_defaults(func=watch)

    p = sub.add_parser("tournament", help="play every pair of PROGRAMS, resumes from results/runs.csv")
    p.add_argument("--games", type=int, default=1000, help="games per pair, split evenly between start orders")
    p.add_argument("--jobs", type=int, default=8, help="parallel DOSBox instances (default 8)")
    p.add_argument("--max-steps", type=int, default=600000, help="MARS /M war length (default 600000)")
    p.add_argument("--cycles", default="fixed 2000000", help="DOSBox CPU cycles (default 'fixed 2000000')")
    p.set_defaults(func=tournament)

    args = parser.parse_args()
    args.func(args)


if __name__ == "__main__":
    main()
