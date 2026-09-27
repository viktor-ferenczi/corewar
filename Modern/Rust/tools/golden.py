#!/usr/bin/env python3
"""Record the outputs of the original MARS.COM under DOSBox as reference data for the Rust tests.

cases     write tests/golden/battle/CASES.txt, the list of recorded battles
compile   compile tests/golden/compile/*.CWR and the Historical programs with a copy of MARS.COM
          patched to dump the compiled programs to a file, store NAME.OUT and NAME.BIN
battle    play the battles of CASES.txt with copies of MARS.COM patched to a fixed random seed,
          store ID.OUT (standard output), ID.LOG (binary statistics) and SESSIONS.txt

MARS.COM itself is never modified, the patched copies live in temporary folders. Needs DOSBox
0.74 and nasm.
"""

import argparse
import hashlib
import random
import re
import shutil
import subprocess
import sys
import tempfile
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

HERE = Path(__file__).resolve().parent
RUST = HERE.parent
GOLDEN = RUST / "tests" / "golden"
HISTORICAL = RUST.parent.parent / "Historical"
MARS_MD5 = "a16f6bf1c8866c2c698cc1f310b844da"

# File offsets in MARS.COM (loaded at 100H).
RANDOMIZE_SEED = 0x1566  # MOV AX,[046CH] / MOV BX,[046EH]
ENTRY_CALL_WAR = 0xA84  # MOV AX,13H / INT 10H / CALL WAR
USAGE_TEXT = 0x136  # M_NOPROG at 236H, 1288 bytes

# The programs of the tournament in Reproduction/mars.py, plus the ones it left out and the test programs.
sys.path.insert(0, str(RUST.parent.parent / "Reproduction"))
from mars import PROGRAMS as TOURNAMENT  # noqa: E402

EXTRA = ["NONE.CWR", "TEST.CWR", "ROHANO.CWR"]
SPECIAL = sorted(p.name for p in (GOLDEN / "progs").glob("*.CWR"))


def mars(seed: int | None = None, dump: bool = False) -> bytes:
    data = bytearray((HISTORICAL / "MARS.COM").read_bytes())
    assert hashlib.md5(data).hexdigest() == MARS_MD5, "unexpected MARS.COM"
    if seed is not None:
        assert data[RANDOMIZE_SEED : RANDOMIZE_SEED + 7] == bytes.fromhex("A16C048B1E6E04")
        lo, hi = seed & 0xFFFF, seed >> 16
        data[RANDOMIZE_SEED : RANDOMIZE_SEED + 7] = (
            b"\xb8" + lo.to_bytes(2, "little") + b"\xbb" + hi.to_bytes(2, "little") + b"\x90"
        )
    if dump:
        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp) / "dump.bin"
            subprocess.run(["nasm", "-f", "bin", "-o", str(out), str(HERE / "dump.asm")], check=True)
            code = out.read_bytes()
        assert len(code) < 1288
        data[USAGE_TEXT : USAGE_TEXT + len(code)] = code
        assert data[ENTRY_CALL_WAR : ENTRY_CALL_WAR + 8] == bytes.fromhex("B81300CD10E8810B")
        rel = 0x236 - (0x100 + ENTRY_CALL_WAR + 3)
        data[ENTRY_CALL_WAR : ENTRY_CALL_WAR + 8] = b"\xe8" + rel.to_bytes(2, "little", signed=True) + b"\x90" * 5
    return bytes(data)


def copy_dos(source: Path, target: Path) -> None:
    """Copy a source with DOS line ends: MARS.COM only ends a line at a CR, and a git checkout may
    have LF line ends."""
    target.write_bytes(re.sub(rb"(?<!\r)\n", b"\r\n", source.read_bytes()))


def dosbox(workdir: Path, commands: list[str], timeout: float) -> bool:
    """Run DOS commands headless with workdir as C:, return False on timeout."""
    conf = f"""[dosbox]
machine=vgaonly
memsize=4
[cpu]
core=dynamic
cycles=max
[mixer]
nosound=true
[speaker]
pcspeaker=false
tandy=off
[midi]
mpu401=none
[autoexec]
mount c "{workdir}"
c:
{chr(10).join(commands)}
exit
"""
    (workdir / "dosbox.conf").write_text(conf.replace("\n", "\r\n"))
    env = {"SDL_VIDEODRIVER": "dummy", "SDL_AUDIODRIVER": "dummy", "PATH": "/usr/bin:/bin"}
    proc = subprocess.Popen(
        ["dosbox", "-conf", str(workdir / "dosbox.conf")], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL
    )
    try:
        proc.wait(timeout=timeout)
        return True
    except subprocess.TimeoutExpired:
        proc.kill()
        proc.wait()
        return False


def compile_golden(args: argparse.Namespace) -> None:
    out = GOLDEN / "compile"
    sources = {p.name: p for p in sorted(out.glob("*.CWR"))}
    historical = {p.name: p for p in sorted(HISTORICAL.iterdir()) if p.name in TOURNAMENT + EXTRA}
    batches = [[n] for n in sources if n.startswith("X")] + [
        [n for n in sources if not n.startswith("X")],
        list(historical),
    ]
    for batch in batches:
        with tempfile.TemporaryDirectory(prefix="golden-") as tmp:
            work = Path(tmp)
            (work / "MARSD.COM").write_bytes(mars(dump=True))
            commands = []
            for n, name in enumerate(batch):
                copy_dos(sources.get(name) or historical[name], work / name)
                commands += [f"MARSD {name} > O{n}.TXT", f"if exist DUMP.BIN ren DUMP.BIN B{n}.BIN"]
            finished = dosbox(work, commands, timeout=30)
            for n, name in enumerate(batch):
                target = out if name in sources else out / "historical"
                target.mkdir(exist_ok=True)
                stem = target / Path(name).stem.replace("-", "_")
                text = (work / f"O{n}.TXT").read_bytes() if (work / f"O{n}.TXT").exists() else b""
                stem.with_suffix(".OUT").write_bytes(text.replace(b"\r\n", b"\n"))
                dump = work / f"B{n}.BIN"
                stem.with_suffix(".BIN").unlink(missing_ok=True)
                if dump.exists():
                    shutil.copy(dump, stem.with_suffix(".BIN"))
                stem.with_suffix(".HNG").unlink(missing_ok=True)
                if not finished:
                    stem.with_suffix(".HNG").write_text("MARS.COM did not finish within the time limit\n")
            print(f"{', '.join(batch)}: {'done' if finished else 'timed out'}")


def cases(args: argparse.Namespace) -> None:
    rng = random.Random(1993)
    seed = lambda: rng.randrange(0x1800B0)  # noqa: E731
    lines = ["# ID SEED OPTIONS PROGRAMS, recorded by tools/golden.py battle"]
    n = 0

    def case(options: str, *programs: str) -> None:
        nonlocal n
        n += 1
        lines.append(f"B{n:03d} {seed()} {options} {' '.join(programs)}")

    count = len(TOURNAMENT)
    for i, p in enumerate(TOURNAMENT):
        case("/P=20", p, TOURNAMENT[(i + 1) % count])
        case("/P=20", TOURNAMENT[(i + 7) % count], p)
    for queue in (1, 2, 8, 100, 256):
        for _ in range(4):
            case(f"/P=20 /Q={queue}", *rng.sample(TOURNAMENT + SPECIAL, 2))
    for _ in range(15):
        case("/P=20 /E", *rng.sample(TOURNAMENT + SPECIAL, 2))
    for k in (3, 3, 3, 3, 3, 3, 3, 3, 4, 4, 5):
        case("/P=10", *rng.sample(TOURNAMENT + SPECIAL, k))
    for p in SPECIAL:
        for _ in range(3):
            case("/P=300 /M=20000", p, rng.choice(TOURNAMENT))
            case("/P=300 /M=20000", rng.choice(TOURNAMENT), p)
    for steps in (1, 2, 3, 10, 999, 1000, 1001, 16001):
        case(f"/P=100 /M={steps}", *rng.sample(TOURNAMENT + SPECIAL, 2))
    for pair in (("IMP.CWR", "IMP.CWR"), ("IMP.CWR", "CHANG2.CWR"), ("CHANG2.CWR", "IMP.CWR"), ("IMP.CWR", "MICE.CWR")):
        case("/P=5", *pair)
    for p in ("MICE.CWR", "IMP.CWR", "FILL.CWR", "LONG.CWR"):
        case("/P=5 /M=50000", p)
    for p in EXTRA:
        case("/P=20", p, rng.choice(TOURNAMENT))
    case("/P=2000 /M=5", "KILLER.CWR", "KILLER2.CWR", "LONG.CWR")
    (GOLDEN / "battle" / "CASES.txt").write_text("\n".join(lines) + "\n")
    print(f"{n} cases")


def parse_cases() -> list[tuple[str, int, list[str], list[str]]]:
    result = []
    for line in (GOLDEN / "battle" / "CASES.txt").read_text().splitlines():
        if line and not line.startswith("#"):
            id_, seed, *rest = line.split()
            result.append(
                (id_, int(seed), [r for r in rest if r.startswith("/")], [r for r in rest if not r.startswith("/")])
            )
    return result


def battle_batch(batch: list[tuple[str, int, list[str], list[str]]]) -> str:
    out = GOLDEN / "battle"
    with tempfile.TemporaryDirectory(prefix="golden-") as tmp:
        work = Path(tmp)
        for name in TOURNAMENT + EXTRA:
            copy_dos(HISTORICAL / name, work / name)
        for name in SPECIAL:
            copy_dos(GOLDEN / "progs" / name, work / name)
        commands = []
        for id_, seed, options, programs in batch:
            (work / f"{id_}.COM").write_bytes(mars(seed=seed))
            commands.append(f"{id_} {' '.join(programs)} {' '.join(options)} /V /F={id_}.LOG > {id_}.OUT")
        finished = dosbox(work, commands, timeout=3600)
        for id_, *_ in batch:
            (out / f"{id_}.OUT").write_bytes((work / f"{id_}.OUT").read_bytes().replace(b"\r\n", b"\n"))
            shutil.copy(work / f"{id_}.LOG", out / f"{id_}.LOG")
        return f"{batch[0][0]}..{batch[-1][0]}: {'done' if finished else 'timed out'}"


def write_sessions(batches: list[list[str]]) -> None:
    """SESSIONS.txt lists the cases played in one DOSBox, in order. MARS does not clear the memory
    after the arena and DOS does not clear it between programs, so a battle depends on the ones
    played before it in the same session."""
    path = GOLDEN / "battle" / "SESSIONS.txt"
    recorded = {id_ for batch in batches for id_ in batch}
    old = [line.split() for line in path.read_text().splitlines()] if path.exists() else []
    sessions = [[id_ for id_ in s if id_ not in recorded] for s in old] + batches
    path.write_text("".join(" ".join(s) + "\n" for s in sessions if s))


def battle(args: argparse.Namespace) -> None:
    all_cases = [c for c in parse_cases() if not args.only or c[0] in args.only]
    batches = [b for b in (all_cases[i :: args.jobs] for i in range(args.jobs)) if b]
    write_sessions([[c[0] for c in b] for b in batches])
    with ThreadPoolExecutor(args.jobs) as pool:
        for result in pool.map(battle_batch, batches):
            print(result, flush=True)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(required=True)
    sub.add_parser("cases", help="write CASES.txt").set_defaults(func=cases)
    sub.add_parser("compile", help="record compiler outputs").set_defaults(func=compile_golden)
    p = sub.add_parser("battle", help="record battle statistics")
    p.add_argument("--jobs", type=int, default=8)
    p.add_argument("only", nargs="*", help="case IDs to record, all by default")
    p.set_defaults(func=battle)
    args = parser.parse_args()
    args.func(args)


if __name__ == "__main__":
    main()
