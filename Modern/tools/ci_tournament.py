"""Check the short historical workload against its recorded baseline results."""

import argparse
import hashlib
import json
import subprocess
import tempfile
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
PROGRAMS = ["IMP.CWR", "MICE.CWR", "CHANG.CWR", "TORPE.CWR", "KILLER.CWR", "KILLER2.CWR", "PRB004.CWR", "Y.CWR"]
# All 56 run results, including placements, scores, process counts and steps.
EXPECTED = "86b05b270290de77694a49ff68980cbc3e797227aabfdf6cfc8c00c2794dadd0"


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path)
    args = parser.parse_args()
    started = time.monotonic()
    with tempfile.TemporaryDirectory(prefix="mars-ci-") as directory:
        output = Path(directory) / "results.jsonl"
        command = [
            str(args.binary.resolve()),
            "tournament",
            "--standard",
            "hu93",
            "--quirks",
            "--norotate",
            "--games",
            "128",
            "--jobs",
            "4",
            "--seed",
            "1",
            "--out",
            str(output),
        ]
        command.extend(str(ROOT / "Historical" / name) for name in PROGRAMS)
        subprocess.run(command, check=True, timeout=60)
        rows = [json.loads(line) for line in output.read_text().splitlines()]
    digest = hashlib.sha256(json.dumps(rows, sort_keys=True, separators=(",", ":")).encode()).hexdigest()
    if digest != EXPECTED:
        raise SystemExit(f"Tournament differs from the historical baseline: {digest}")
    print(f"All 3584 wars match the baseline in {time.monotonic() - started:.2f} s")


if __name__ == "__main__":
    main()
