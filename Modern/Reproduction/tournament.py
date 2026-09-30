#!/usr/bin/env python3
"""Play the round robin of Reproduction/mars.py on the native Rust engine.

Same programs, same order, 1000 games per pair (500 per start order), the MARS defaults. Writes
results/runs.jsonl, one JSON object per run, and records the wall clock time and machine in
results/timing.json, one entry per backend (cpu or gpu): the results are the same on both.

With --baseline the tournament is played once more with another master seed into
results/baseline.jsonl. The report compares the two to show how much two tournaments differ by
chance alone.
"""

import argparse
import json
import os
import platform
import subprocess
import sys
import time
from datetime import date
from pathlib import Path

HERE = Path(__file__).resolve().parent
RUST = HERE.parent
REPRODUCTION = RUST.parent / "Reproduction"
RESULTS = HERE / "results"
TIMING = RESULTS / "timing.json"
MARS = RUST / "target" / "release" / "mars"
sys.path.insert(0, str(REPRODUCTION))
from mars import HISTORICAL, PROGRAMS  # noqa: E402


def cpu_model() -> str:
    for line in Path("/proc/cpuinfo").read_text().splitlines():
        if line.startswith("model name"):
            return line.split(":", 1)[1].strip()
    return platform.processor()


def gpu_names(devices: str) -> list[str]:
    """Names of the GPUs of a --gpu list, from `mars gpus`."""
    listing = subprocess.run(
        [MARS, "gpus"], capture_output=True, text=True, check=True
    ).stdout
    # Lines of "INDEX  NAME  (KIND, BACKEND, DRIVER)", the best kind first.
    rows = [line.split("  ") for line in listing.splitlines()]
    if devices == "all":
        kind = rows[0][2].split(",")[0]
        return [r[1] for r in rows if r[2].split(",")[0] == kind]
    return [rows[int(d)][1] for d in devices.split(",")]


def main() -> None:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument(
        "--gpu",
        metavar="LIST",
        help="play on GPUs (e.g. 0, 0,1 or all) instead of the CPU",
    )
    parser.add_argument(
        "--jobs",
        type=int,
        default=os.cpu_count(),
        help="CPU threads (default: all cores)",
    )
    parser.add_argument(
        "--seed",
        type=int,
        default=1993,
        help="master seed of the per run tick counts (1993)",
    )
    parser.add_argument(
        "--baseline",
        type=int,
        metavar="SEED",
        help="only play the baseline with this master seed",
    )
    args = parser.parse_args()

    subprocess.run(["cargo", "build", "--release", "--quiet"], cwd=RUST, check=True)
    RESULTS.mkdir(exist_ok=True)
    out = RESULTS / ("baseline.jsonl" if args.baseline is not None else "runs.jsonl")
    command = [
        MARS,
        "tournament",
        "--standard",
        "hu93",
        "--quirks",
        "--norotate",
        "--games",
        "1000",
        "--out",
        out,
    ]
    command += [
        "--seed",
        str(args.baseline if args.baseline is not None else args.seed),
    ]
    command += ["--gpu", args.gpu] if args.gpu else ["--jobs", str(args.jobs)]
    started = time.monotonic()
    subprocess.run(command + [HISTORICAL / p for p in PROGRAMS], check=True)
    seconds = time.monotonic() - started
    if args.baseline is not None:
        return

    rustc = subprocess.run(
        ["rustc", "--version"], capture_output=True, text=True, check=True
    ).stdout.strip()
    timing = json.loads(TIMING.read_text()) if TIMING.exists() else {}
    entry = {
        "date": date.today().isoformat(),
        "seconds": round(seconds, 1),
        "seed": args.seed,
        "rustc": rustc,
    }
    if args.gpu:
        timing["gpu"] = entry | {"devices": gpu_names(args.gpu)}
    else:
        timing["cpu"] = entry | {
            "jobs": args.jobs,
            "cpu": cpu_model(),
            "logical_cpus": os.cpu_count(),
        }
    TIMING.write_text(json.dumps(timing, indent=2) + "\n")
    print(f"Tournament finished in {seconds:.1f} s")


if __name__ == "__main__":
    main()
