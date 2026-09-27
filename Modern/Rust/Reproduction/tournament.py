#!/usr/bin/env python3
"""Play the round robin of Reproduction/mars.py on the native Rust engine.

Same programs, same order, 500 games per start order, the MARS defaults. Writes raw/*.sta,
results/runs.csv and results/timing.json (wall clock time and machine) next to this script.

With --baseline the tournament is played once more with another master seed, and only its runs.csv
is kept as results/baseline.csv. The report compares the two to show how much two tournaments differ
by chance alone.
"""

import argparse
import json
import os
import platform
import shutil
import subprocess
import sys
import tempfile
import time
from datetime import date
from pathlib import Path

HERE = Path(__file__).resolve().parent
RUST = HERE.parent
REPRODUCTION = RUST.parent.parent / "Reproduction"
sys.path.insert(0, str(REPRODUCTION))
from mars import HISTORICAL, PROGRAMS  # noqa: E402


def cpu_model() -> str:
    for line in Path("/proc/cpuinfo").read_text().splitlines():
        if line.startswith("model name"):
            return line.split(":", 1)[1].strip()
    return platform.processor()


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--jobs", type=int, default=os.cpu_count(), help="parallel threads (default: all cores)")
    parser.add_argument("--seed", type=int, default=1993, help="master seed of the per run tick counts (1993)")
    parser.add_argument("--baseline", type=int, metavar="SEED", help="only play the baseline with this master seed")
    args = parser.parse_args()

    subprocess.run(["cargo", "build", "--release", "--quiet"], cwd=RUST, check=True)

    def play(out: Path, seed: int) -> None:
        command = [str(RUST / "target" / "release" / "mars"), "tournament", "--out", str(out)]
        command += ["--wars", "500", "--jobs", str(args.jobs), "--seed", str(seed)]
        subprocess.run(command + [str(HISTORICAL / p) for p in PROGRAMS], check=True)

    if args.baseline is not None:
        with tempfile.TemporaryDirectory() as tmp:
            play(Path(tmp), args.baseline)
            (HERE / "results").mkdir(exist_ok=True)
            shutil.copy(Path(tmp) / "results" / "runs.csv", HERE / "results" / "baseline.csv")
        return
    started = time.monotonic()
    play(HERE, args.seed)
    seconds = time.monotonic() - started
    rustc = subprocess.run(["rustc", "--version"], capture_output=True, text=True, check=True).stdout.strip()
    timing = {
        "date": date.today().isoformat(),
        "seconds": round(seconds, 1),
        "jobs": args.jobs,
        "seed": args.seed,
        "cpu": cpu_model(),
        "logical_cpus": os.cpu_count(),
        "rustc": rustc,
    }
    (HERE / "results" / "timing.json").write_text(json.dumps(timing, indent=2) + "\n")
    print(f"Tournament finished in {seconds:.1f} s")


if __name__ == "__main__":
    main()
