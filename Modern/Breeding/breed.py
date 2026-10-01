#!/usr/bin/env python3
"""Breed Redcode warriors: a genetic algorithm evaluated by gauntlet tournaments of the Rust MARS.

  breed.py iterate RUN   one inner run: take the inbox, breed for --minutes, write a report
  breed.py rank [RUN]    full tournament of the benchmark field, with the run's best warriors
  breed.py pmars [RUN]   replay sample pairs on pMARS (PMARS_092_BIN) as an independent check

Warriors play under the engine's default rules (--standard pmars: ICWS'94 without P-space, core
8000). The benchmark warriors are opponents only, see benchmark/USAGE.md. See README.md and
AGENT.md.
"""

import argparse
import difflib
import gzip
import importlib.util
import json
import math
import os
import random
import re
import secrets
import shutil
import subprocess
import time
from datetime import datetime, timedelta
from pathlib import Path

from benchmark.fetch import WARRIORS, field_summary, load_field, log_usage

HERE = Path(__file__).resolve().parent
MARS = Path(os.environ.get("MARS_BIN", HERE.parent / "target" / "release" / "mars"))
RUNS = HERE / "runs"
SEEDS = HERE / "seeds"
BENCHMARK = HERE / "benchmark"

CORE = 8000
MAX_LENGTH = 100
# No P-space (LDP, STP), and SEQ stands for its synonym CMP.
OPCODES = [
    "DAT",
    "MOV",
    "ADD",
    "SUB",
    "MUL",
    "DIV",
    "MOD",
    "JMP",
    "JMZ",
    "JMN",
    "DJN",
    "SPL",
    "SLT",
    "SEQ",
    "SNE",
    "NOP",
]
MODIFIERS = ["A", "B", "AB", "BA", "F", "X", "I"]
MODES = "#$*@{<}>"
# MAP-Elites cells: upper bounds of the program length and of the mean processes left per war.
LENGTH_BINS = [5, 10, 20, 40, MAX_LENGTH]
PROCESS_BINS = [1, 2, 10, 100, math.inf]

DEFAULTS = {
    "population": 200,  # new candidates per generation
    "screen_games": 40,  # games per pair, half with each first mover
    "refine_games": 400,
    "hall_games": 2000,
    "refine_fraction": 0.2,
    "hall_size": 20,
    "generation_seconds": 120,  # the stages shrink to fit this on slower hardware
    "opponents": 0,  # benchmark warriors to play against, 0 for all
    "seeds": True,  # start from seeds/*.red
    "hu93_seeds": [],  # MARS.COM sources to start from, compiled with --syntax hu93
}


# --- the engine --------------------------------------------------------------------------------


def mars(*args: str) -> subprocess.CompletedProcess:
    if not MARS.exists():
        raise SystemExit(f"{MARS} is missing, build it with: cargo build --release")
    return subprocess.run([str(MARS), *args], capture_output=True, text=True)


def discrete_gpus() -> list[int]:
    """Indexes of the discrete GPUs in `mars gpus`. A CPU-only build has none."""
    listing = mars("gpus")
    if listing.returncode != 0:
        return []
    return [int(m[1]) for m in re.finditer(r"^(\d+)\s.*\(DiscreteGpu,", listing.stdout, re.MULTILINE)]


def device_args(override: str | None) -> list[str]:
    """Every discrete GPU, else the CPU. Never an integrated or a software GPU, not even by override."""
    override = override or os.environ.get("BREED_GPUS")
    if override == "cpu":
        return ["--cpu"]
    discrete = discrete_gpus()
    wanted = [int(i) for i in override.split(",")] if override else discrete
    if not set(wanted) <= set(discrete):
        raise SystemExit(f"--gpu {override}: breeding only uses discrete GPUs, mars gpus lists {discrete} as such")
    return ["--gpu", ",".join(map(str, wanted))] if wanted else ["--cpu"]


def compile_sources(paths: list[Path], hu93: bool = False) -> list[dict]:
    """One result per path: the code and start of the assembled program, or its error."""
    if not paths:
        return []
    output = mars("compile", "--json", *(["--syntax", "hu93"] if hu93 else []), *map(str, paths)).stdout
    results = []
    for line in output.splitlines():
        compiled = json.loads(line)
        if compiled["ok"]:
            fields = ("op", "modifier", "a_mode", "a", "b_mode", "b")
            code = [[ins[f] for f in fields] for ins in compiled["instructions"]]
            results.append({"code": code, "start": compiled["start"]})
        else:
            results.append({"error": "; ".join(compiled["errors"])})
    return results


def canonical(warrior: dict) -> str:
    """The source that plays: explicit modifiers, numeric addresses, no labels. Runs on pMARS as is."""
    signed = lambda v: v if v <= CORE // 2 else v - CORE
    meta = {k: warrior.get(k) for k in ("op", "parents", "gen", "intent")}
    lines = [
        ";redcode-94nop",
        f";name {warrior['id']}",
        ";author corewar Modern/Breeding",
        f";breeding {json.dumps(meta)}",
        f";assert CORESIZE=={CORE}",
        f"ORG {warrior['start']}",
    ]
    lines += [f"{op}.{mod} {am}{signed(a)}, {bm}{signed(b)}" for op, mod, am, a, bm, b in warrior["code"]]
    return "\n".join(lines) + "\n"


class Tally:
    """Wins, draws, losses and processes left of one warrior over some opponents."""

    def __init__(self, rows=()):
        self.wins, self.draws, self.losses, self.pcs = (sum(col) for col in zip(*rows)) if rows else (0, 0, 0, 0)

    @property
    def games(self) -> int:
        return self.wins + self.draws + self.losses

    @property
    def mean(self) -> float:
        return (self.wins + self.draws / 2) / self.games

    @property
    def error(self) -> float:
        """Standard error of the mean score."""
        variance = (self.wins + self.draws / 4) / self.games - self.mean**2
        return math.sqrt(max(variance, 0) / self.games)

    @property
    def koth(self) -> float:
        """3 points per win and 1 per draw, per 100 games, as the hills score."""
        return (3 * self.wins + self.draws) / self.games * 100

    @property
    def processes(self) -> float:
        return self.pcs / self.games

    def __str__(self) -> str:
        return f"{self.mean:.3f} ± {1.96 * self.error:.3f}"


def z_score(a: Tally, b: Tally) -> float:
    spread = math.hypot(a.error, b.error)
    return (a.mean - b.mean) / spread if spread else 0.0


def gauntlet(
    out: Path, candidates: list[Path], opponents: list[Path], games: int, seed: int, device: list[str]
) -> tuple[dict[str, dict[str, list[int]]], int, float]:
    """Play every candidate against every opponent.

    Returns [wins, draws, losses, processes left] by candidate and opponent file name, the wars
    played and the seconds taken. The raw results stay next to `out`, compressed.
    """
    started = time.monotonic()
    names = [str(p) for p in candidates] + ["--against"] + [str(p) for p in opponents]
    played = mars("tournament", "--games", str(games), "--seed", str(seed), "--out", str(out), *device, *names)
    if played.returncode != 0:
        raise SystemExit(f"mars tournament failed:\n{played.stderr[-2000:]}")
    seconds = time.monotonic() - started
    raw = out.read_text()
    with gzip.open(f"{out}.gz", "wt") as f:
        f.write(raw)
    out.unlink()
    ours = {p.name for p in candidates}
    results: dict[str, dict[str, list[int]]] = {name: {} for name in ours}
    for line in raw.splitlines():
        r = json.loads(line)
        me, other = ("first", "second") if r["first"] in ours else ("second", "first")
        results[r[me]][r[other]] = [r[f"{me}_wins"], r["draws"], r[f"{other}_wins"], r[f"{me}_pcs"]]
    return results, len(candidates) * len(opponents) * games, seconds


# --- genetic operators -------------------------------------------------------------------------


def random_value(rng: random.Random, old: int | None = None) -> int:
    """Near the old value, a small offset, or a step coprime with the core size."""
    kind = rng.random()
    if old is not None and kind < 0.4:
        return (old + rng.randint(-5, 5)) % CORE
    if kind < 0.7:
        return rng.randint(-20, 20) % CORE
    while True:
        step = rng.randrange(1, CORE)
        if math.gcd(step, CORE) == 1:
            return step


def random_instruction(rng: random.Random) -> list:
    return [
        rng.choice(OPCODES),
        rng.choice(MODIFIERS),
        rng.choice(MODES),
        random_value(rng),
        rng.choice(MODES),
        random_value(rng),
    ]


def random_program(rng: random.Random) -> tuple[list, int]:
    code = [random_instruction(rng) for _ in range(rng.randint(1, 20))]
    return code, rng.randrange(len(code))


def mutate(rng: random.Random, code: list, start: int) -> tuple[list, int]:
    """One to three point mutations, or an inserted, deleted or duplicated instruction."""
    code = [list(ins) for ins in code]
    for _ in range(rng.randint(1, 3)):
        at = rng.randrange(len(code))
        kind = rng.random()
        if kind < 0.10 and len(code) < MAX_LENGTH:
            code.insert(at, random_instruction(rng))
        elif kind < 0.20 and len(code) < MAX_LENGTH:
            code.insert(at, list(code[at]))
        elif kind < 0.30 and len(code) > 1:
            del code[at]
        elif kind < 0.40:
            code[at][0] = rng.choice(OPCODES)
        elif kind < 0.50:
            code[at][1] = rng.choice(MODIFIERS)
        elif kind < 0.60:
            code[at][rng.choice((2, 4))] = rng.choice(MODES)
        else:
            field = rng.choice((3, 5))
            code[at][field] = random_value(rng, code[at][field])
    return code, min(start, len(code) - 1)


def crossover(rng: random.Random, a: list, b: list, start: int) -> tuple[list, int]:
    """One-point: a head of `a` and a tail of `b`. Two-point: a slice of `b` inside `a`."""
    cut = rng.randint(0, len(a))
    if rng.random() < 0.5:
        code = a[:cut] + b[rng.randint(0, len(b)) :]
    else:
        begin, end = sorted((rng.randint(0, len(b)), rng.randint(0, len(b))))
        code = a[:cut] + b[begin:end] + a[rng.randint(cut, len(a)) :]
    code = [list(ins) for ins in code[:MAX_LENGTH]] or [list(a[0])]
    return code, min(start, len(code) - 1)


def sweep(rng: random.Random, code: list, count: int = 8) -> list[list]:
    """The same warrior with one constant varied, preferably the step of an ADD or SUB."""
    steps = [(i, 3) for i, ins in enumerate(code) if ins[0] in ("ADD", "SUB") and ins[2] == "#"]
    at, field = rng.choice(steps) if steps else (rng.randrange(len(code)), rng.choice((3, 5)))
    variants = []
    for _ in range(count):
        variant = [list(ins) for ins in code]
        variant[at][field] = random_value(rng, variant[at][field])
        variants.append(variant)
    return variants


def breed(rng: random.Random, parents: list[dict], count: int) -> list[dict]:
    """New warriors from the parents, or random programs when there are none yet."""
    children: list[dict] = []
    seen = {json.dumps(p["code"]) for p in parents}

    def add(code, start, op, *bred_from):
        if json.dumps(code) in seen:
            return
        seen.add(json.dumps(code))
        children.append({"code": code, "start": start, "op": op, "parents": [p["id"] for p in bred_from]})

    while len(children) < count:
        if not parents:
            add(*random_program(rng), "random")
            continue
        parent = rng.choice(parents)
        kind = rng.random()
        if kind < 0.15:
            for variant in sweep(rng, parent["code"]):
                add(variant, parent["start"], "sweep", parent)
        elif kind < 0.35 and len(parents) > 1:
            other = rng.choice([p for p in parents if p is not parent])
            add(*crossover(rng, parent["code"], other["code"], parent["start"]), "cross", parent, other)
        else:
            add(*mutate(rng, parent["code"], parent["start"]), "mutate", parent)
    return children[:count]


# --- a run -------------------------------------------------------------------------------------


def cell_of(length: int, processes: float) -> str:
    bin_of = lambda bins, value: next(i for i, bound in enumerate(bins) if value <= bound)
    return f"{bin_of(LENGTH_BINS, length)}-{bin_of(PROCESS_BINS, processes)}"


class Run:
    def __init__(self, name: str, options: dict, gpu: str | None):
        self.dir = RUNS / name
        self.name = name
        config_path = self.dir / "config.json"
        if config_path.exists():
            self.config = json.loads(config_path.read_text())
        else:
            self.config = {
                **DEFAULTS,
                "seed": secrets.randbits(48),
                "created": datetime.now().isoformat("T", "seconds"),
            }
            self.config.update({k: v for k, v in options.items() if v is not None})
            hours = self.config.pop("total_hours", None)
            if hours:
                self.config["deadline"] = (datetime.now() + timedelta(hours=hours)).isoformat("T", "seconds")
            for folder in ("inbox", "hof", "reports", "llm"):
                (self.dir / folder).mkdir(parents=True, exist_ok=True)
            config_path.write_text(json.dumps(self.config, indent=1) + "\n")
            (self.dir / "NOTEBOOK.md").write_text(f"# Lab notebook of run {name}\n")
        state_path = self.dir / "state.json"
        self.state = (
            json.loads(state_path.read_text())
            if state_path.exists()
            else {"next_id": 1, "generation": 0, "iteration": 0, "archive": {}, "hof": [], "iterations": []}
        )
        self.device = device_args(gpu)
        self.field = load_field()
        warriors = self.field["warriors"][: self.config["opponents"] or None]
        self.benchmark = [WARRIORS / w["file"] for w in warriors]
        missing = [p.name for p in self.benchmark if not p.exists()]
        if missing:
            raise SystemExit(f"benchmark warriors missing ({', '.join(missing[:3])}...), run benchmark/fetch.py")
        self.credits = {w["file"]: f"{w['name']} by {w['author']}" for w in warriors}
        self.rng = random.Random(f"{self.config['seed']}-{self.state['iteration'] + 1}")
        self.games = {stage: self.config[f"{stage}_games"] for stage in ("screen", "refine")}
        self.wars = 0
        self.seconds = 0.0
        self.seconds_per_generation = 0.0
        self.events: list[dict] = []
        self.llm: list[dict] = []
        self.notes: list[str] = []

    def save(self) -> None:
        (self.dir / "state.json").write_text(json.dumps(self.state) + "\n")

    def adopt(self, warrior: dict) -> dict:
        """Give a new warrior its ID and generation."""
        warrior["id"] = f"w{self.state['next_id']:06d}"
        warrior["gen"] = self.state["generation"]
        self.state["next_id"] += 1
        return warrior

    def hof_files(self) -> list[Path]:
        return [self.dir / "hof" / f"hof-{w['id']}.red" for w in self.state["hof"]]

    def play(self, folder: Path, stage: str, warriors: list[dict], opponents: list[Path], games: int) -> dict:
        """Gauntlet of the warriors under a new master seed. Returns the results by warrior ID."""
        folder.mkdir(parents=True, exist_ok=True)
        files = []
        for warrior in warriors:
            files.append(folder / f"{warrior['id']}.red")
            files[-1].write_text(canonical(warrior))
        seed = self.rng.getrandbits(48)
        results, wars, seconds = gauntlet(folder / f"{stage}.jsonl", files, opponents, games, seed, self.device)
        with (folder / "seeds.txt").open("a") as f:
            f.write(f"{stage} games={games} seed={seed} wars={wars} seconds={seconds:.1f}\n")
        self.wars += wars
        self.seconds += seconds
        return {name.removesuffix(".red"): rows for name, rows in results.items()}

    def load_sources(self, paths: list[Path], op: str, hu93: bool = False) -> list[dict]:
        """Assemble sources into warriors. A source that fails comes back with its error."""
        loaded = []
        for path, compiled in zip(paths, compile_sources(paths, hu93)):
            intent = re.search(r"^;(?:intent|strategy)[ \t]+(.*?)\s*$", path.read_text("latin-1"), re.MULTILINE)
            entry = {"file": path.name, "op": op, "parents": [], "intent": intent[1] if intent else "", **compiled}
            if "error" in entry:
                entry["error"] = entry["error"].replace(str(path), path.name)
            loaded.append(entry)
        return loaded

    def take_inbox(self) -> list[dict]:
        """The LLM's candidates. The annotated originals move to llm/ next to their canonical IDs."""
        paths = sorted(p for p in (self.dir / "inbox").iterdir() if p.is_file())
        kept = self.dir / "llm" / f"iter-{self.state['iteration']:03d}"
        candidates = []
        for path, entry in zip(paths, self.load_sources(paths, "llm")):
            kept.mkdir(parents=True, exist_ok=True)
            shutil.move(path, kept / path.name)
            if "error" not in entry:
                candidates.append(entry)
            self.llm.append(entry)
        return candidates

    def seeds(self) -> list[dict]:
        seeds = self.load_sources(sorted(SEEDS.glob("*.red")), "seed") if self.config["seeds"] else []
        seeds += self.load_sources([Path(p) for p in self.config["hu93_seeds"]], "seed", hu93=True)
        for seed in seeds:
            if "error" in seed:
                raise SystemExit(f"seed {seed['file']}: {seed['error']}")
        return seeds

    def fit_to_hardware(self, wars: int, seconds: float, candidates: int) -> None:
        """After the first screen: fewer games per pair if a generation would take too long."""
        per_candidate = len(self.benchmark) + len(self.state["hof"])
        planned = (
            candidates * per_candidate * (self.games["screen"] + self.config["refine_fraction"] * self.games["refine"])
        )
        scale = self.config["generation_seconds"] / (planned / (wars / seconds))
        if scale < 1:
            self.games = {
                "screen": max(10, int(self.games["screen"] * scale)),
                "refine": max(40, int(self.games["refine"] * scale)),
            }
            self.notes.append(f"Slow hardware ({wars / seconds:.0f} wars/s): games per pair cut to {self.games}.")

    def generation(self, newcomers: list[dict]) -> None:
        state, config = self.state, self.config
        state["generation"] += 1
        folder = self.dir / "gen" / f"{state['generation']:04d}"
        elites = list(state["archive"].values())
        room = max(config["population"] - len(newcomers), 0)
        children = [self.adopt(w) for w in newcomers + breed(self.rng, elites, room)]
        everyone = elites + children
        opponents = self.benchmark + self.hof_files()
        benchmark = [p.name for p in self.benchmark]

        screen = self.play(folder, "screen", everyone, opponents, self.games["screen"])
        if not self.seconds_per_generation:  # the first generation of this inner run
            self.fit_to_hardware(self.wars, self.seconds, len(everyone))
        fitness = {w["id"]: Tally(screen[w["id"]].values()).mean for w in everyone}
        ranked = sorted(everyone, key=lambda w: -fitness[w["id"]])
        finalists = ranked[: max(1, math.ceil(config["refine_fraction"] * len(ranked)))]
        refine = self.play(folder, "refine", finalists, opponents, self.games["refine"])
        fitness.update({w["id"]: Tally(refine[w["id"]].values()).mean for w in finalists})

        for elite in elites:
            elite["fitness"] = fitness[elite["id"]]
        refined = {w["id"] for w in finalists}
        with (self.dir / "lineage.jsonl").open("a") as f:
            for child in children:
                row = {k: child.get(k) for k in ("id", "gen", "op", "parents", "intent", "file")}
                row["screen"] = round(Tally(screen[child["id"]].values()).mean, 4)
                row["refine"] = round(fitness[child["id"]], 4) if child["id"] in refined else None
                f.write(json.dumps(row) + "\n")

        by_id = {w["id"]: w for w in everyone}
        newcomers_refined = [w for w in finalists if w in children]
        for child in newcomers_refined:
            tally = Tally(refine[child["id"]].values())
            child["fitness"] = fitness[child["id"]]
            child["benchmark"] = Tally(refine[child["id"]][o] for o in benchmark).mean
            child["cell"] = cell_of(len(child["code"]), tally.processes)
            holder = state["archive"].get(child["cell"])
            if holder and holder["fitness"] >= child["fitness"]:
                continue
            state["archive"][child["cell"]] = child
            parents = [by_id[p] for p in child["parents"] if p in by_id]
            if parents:
                best = max(parents, key=lambda p: fitness[p["id"]])
                diff = difflib.unified_diff(
                    canonical(best).splitlines()[5:],
                    canonical(child).splitlines()[5:],
                    best["id"],
                    child["id"],
                    lineterm="",
                    n=1,
                )
                self.events.append(
                    {
                        "id": child["id"],
                        "op": child["op"],
                        "gain": child["fitness"] - fitness[best["id"]],
                        "fitness": child["fitness"],
                        "diff": list(diff)[:14],
                    }
                )
        if newcomers_refined:
            self.try_hall(folder, max(newcomers_refined, key=lambda w: w["benchmark"]))
        self.save()

    def try_hall(self, folder: Path, candidate: dict) -> None:
        """The generation's best newcomer plays the benchmark at full length. While the hall of fame
        has room it joins if it beats the best member, after that it replaces the weakest member if
        it beats that one, in both cases with z > 2."""
        hall, full = self.state["hof"], len(self.state["hof"]) >= self.config["hall_size"]
        strength = lambda w: Tally([w["hall"]]).mean
        rival = (min if full else max)(hall, key=strength, default=None)
        if rival and candidate["benchmark"] <= strength(rival):
            return
        results = self.play(folder, "hall", [candidate], self.benchmark, self.config["hall_games"])
        tally = Tally(results[candidate["id"]].values())
        if rival and z_score(tally, Tally([rival["hall"]])) <= 2:
            return
        weakest = rival if full else None
        if weakest:
            hall.remove(weakest)
            (self.dir / "hof" / f"hof-{weakest['id']}.red").unlink()
        member = {**candidate, "hall": [tally.wins, tally.draws, tally.losses, tally.pcs]}
        hall.append(member)
        (self.dir / "hof" / f"hof-{member['id']}.red").write_text(canonical(member))
        self.notes.append(
            f"Hall of fame: {member['id']} joined in generation {self.state['generation']}"
            + (f", replacing {weakest['id']}." if weakest else ".")
        )

    def iterate(self, minutes: float, generations: int | None) -> Path:
        state = self.state
        state["iteration"] += 1
        purpose = "local breeding benchmark, opponents only"
        played = (
            field_summary(self.field)
            if not self.config["opponents"]
            else f"the first {len(self.benchmark)} warriors of field.json ({self.field['source']}, {self.field['page']})"
        )
        log_usage(f"{self.name}, iteration {state['iteration']}", purpose, played)

        newcomers = self.take_inbox() + (self.seeds() if state["generation"] == 0 else [])
        deadline = time.monotonic() + minutes * 60
        first = state["generation"] + 1
        while True:
            started = time.monotonic()
            self.generation(newcomers)
            newcomers = []
            self.seconds_per_generation = time.monotonic() - started
            done = state["generation"] - first + 1
            print(
                f"generation {state['generation']}: {len(state['archive'])} cells, best {max(w['fitness'] for w in state['archive'].values()):.3f}, {self.seconds_per_generation:.0f} s",
                flush=True,
            )
            if (generations and done >= generations) or time.monotonic() + self.seconds_per_generation > deadline:
                break
        report = self.report(done)
        self.save()
        return report

    # --- the report ---

    def leaderboard(self) -> tuple[list[dict], dict]:
        """The archive and the hall of fame against the benchmark and the hall of fame, on new seeds."""
        best = {w["id"]: w for w in list(self.state["archive"].values()) + self.state["hof"]}
        folder = self.dir / "gen" / f"final-{self.state['iteration']:03d}"
        results = self.play(
            folder, "final", list(best.values()), self.benchmark + self.hof_files(), self.games["refine"]
        )
        benchmark = [p.name for p in self.benchmark]
        hall = [p.name for p in self.hof_files()]
        rows = [
            {
                "warrior": w,
                "benchmark": Tally(results[i][o] for o in benchmark),
                "hall": Tally(results[i][o] for o in hall),
            }
            for i, w in best.items()
        ]
        rows.sort(key=lambda r: -r["benchmark"].mean)
        return rows, results[rows[0]["warrior"]["id"]]

    def stop_rule(self) -> str:
        history = self.state["iterations"]
        deadline = self.config.get("deadline")
        if deadline and datetime.now().isoformat() > deadline:
            return f"STOP: the run's time budget ended at {deadline}."
        if len(history) > 5:
            before = max(history[:-5], key=lambda h: h["mean"])
            recent = max(history[-5:], key=lambda h: h["mean"])
            if recent["mean"] - before["mean"] <= 2 * math.hypot(before["error"], recent["error"]):
                return "STOP: no significant gain against the benchmark field in 5 iterations."
        return "Continue: no stop rule has fired."

    def report(self, generations: int) -> Path:
        state = self.state
        rows, champion_results = self.leaderboard()
        champion = rows[0]
        state["iterations"].append(
            {
                "iteration": state["iteration"],
                "champion": champion["warrior"]["id"],
                "mean": champion["benchmark"].mean,
                "error": champion["benchmark"].error,
                "koth": champion["benchmark"].koth,
            }
        )
        (self.dir / "champion.red").write_text(canonical(champion["warrior"]))
        describe = lambda w: f"{w['id']} ({w['op']}, gen {w['gen']}, {len(w['code'])} instructions)"

        out = [
            f"# Run {self.name}, iteration {state['iteration']}",
            "",
            f"{generations} generations (up to {state['generation']}), {self.wars} wars in {self.seconds:.0f} s"
            f" ({self.wars / self.seconds:.0f} wars/s) on `{' '.join(self.device)}`."
            f" Games per pair: screen {self.games['screen']}, refine {self.games['refine']}, hall {self.config['hall_games']}."
            f" Opponents: {len(self.benchmark)} benchmark warriors and {len(state['hof'])} hall of fame members.",
            "",
            "Score is (wins + draws / 2) / games with its 95% interval. KOTH is 3 points per win and 1 per draw, per 100 games.",
            "",
            *self.notes,
            "",
            "## Leaderboard against the benchmark field",
            "",
            "| # | Warrior | Score | KOTH | W / D / L |",
            "|--:|:--------|:------|-----:|:----------|",
        ]
        for n, row in enumerate(rows[:10], 1):
            t = row["benchmark"]
            out.append(f"| {n} | {describe(row['warrior'])} | {t} | {t.koth:.1f} | {t.wins} / {t.draws} / {t.losses} |")
        if state["hof"]:
            out += [
                "",
                "## Leaderboard against the hall of fame",
                "",
                "A member also plays its own copy.",
                "",
                "| # | Warrior | Score |",
                "|--:|:--------|:------|",
            ]
            by_hall = sorted(rows, key=lambda r: -r["hall"].mean)[:10]
            out += [f"| {n} | {describe(row['warrior'])} | {row['hall']} |" for n, row in enumerate(by_hall, 1)]

        matchups = sorted(
            ((Tally([champion_results[name]]), credit) for name, credit in self.credits.items()),
            key=lambda m: m[0].mean,
        )
        shown = matchups if len(matchups) <= 20 else matchups[:15] + matchups[-5:]
        out += [
            "",
            f"## Champion {champion['warrior']['id']}",
            "",
            "```",
            *canonical(champion["warrior"]).splitlines()[5:45],
            "```",
            "",
        ]
        out += [
            ("Every matchup" if shown is matchups else "Its 15 worst and 5 best matchups") + ", worst first:",
            "",
            "| Opponent | Score | W / D / L |",
            "|:---------|:------|:----------|",
        ]
        out += [f"| {credit} | {t.mean:.3f} | {t.wins} / {t.draws} / {t.losses} |" for t, credit in shown]

        if self.llm:
            lineage = [json.loads(line) for line in (self.dir / "lineage.jsonl").read_text().splitlines()]
            children: dict[str, list[dict]] = {}
            for row in lineage:
                for parent in row["parents"]:
                    children.setdefault(parent, []).append(row)
            score = lambda row: row["refine"] if row["refine"] is not None else row["screen"]

            def best_descendant(warrior_id: str) -> float | None:
                found, queue = [], list(children.get(warrior_id, []))
                while queue:
                    row = queue.pop()
                    found.append(score(row))
                    queue += children.get(row["id"], [])
                return max(found, default=None)

            mine = {
                row["file"]: row
                for row in lineage
                if row["op"] == "llm" and row["gen"] > state["generation"] - generations
            }
            out += [
                "",
                "## LLM candidates",
                "",
                "| File | Result | Score | Best descendant | Intent |",
                "|:-----|:-------|------:|----------------:|:-------|",
            ]
            for entry in self.llm[:20]:
                row = mine.get(entry["file"])
                if not row:
                    out.append(f"| {entry['file']} | rejected: {entry['error'][:90]} | | | {entry['intent'][:60]} |")
                    continue
                below = best_descendant(row["id"])
                out.append(
                    f"| {entry['file']} | {row['id']} | {score(row):.3f} | {'' if below is None else f'{below:.3f}'} | {entry['intent'][:60]} |"
                )

        out += ["", "## Biggest improvements", ""]
        for event in sorted(self.events, key=lambda e: -e["gain"])[:3]:
            out += [
                f"{event['id']} ({event['op']}): {event['fitness']:.3f}, {event['gain']:+.3f} over its best parent.",
                "",
                "```diff",
                *event["diff"],
                "```",
                "",
            ]
        if not self.events:
            out += ["No child entered the archive ahead of its parents.", ""]

        cells = len(state["archive"])
        lengths = sorted({w["cell"].split("-")[0] for w in state["archive"].values()})
        history = ", ".join(f"{h['iteration']}: {h['mean']:.3f}" for h in state["iterations"][-8:])
        out += [
            "## Stagnation and diversity",
            "",
            f"- Archive: {cells} of {len(LENGTH_BINS) * len(PROCESS_BINS)} cells filled, {len(lengths)} of {len(LENGTH_BINS)} length bins."
            + (" LOW DIVERSITY." if cells < 4 else ""),
            f"- Hall of fame: {len(state['hof'])} of {self.config['hall_size']}.",
            f"- Champion's benchmark score by iteration: {history}.",
            f"- {self.stop_rule()}",
        ]
        path = self.dir / "reports" / f"iter-{state['iteration']:03d}.md"
        path.write_text("\n".join(out) + "\n")
        return path


# --- ranking and the pMARS check ---------------------------------------------------------------


def best_of(run: str | None, top: int) -> list[Path]:
    """Canonical files of the run's best hall of fame members."""
    if not run:
        return []
    hall = json.loads((RUNS / run / "state.json").read_text())["hof"]
    hall.sort(key=lambda w: -Tally([w["hall"]]).mean)
    return [RUNS / run / "hof" / f"hof-{w['id']}.red" for w in hall[:top]]


def rank(run: str | None, top: int, games: int, gpu: str | None) -> Path:
    """Full tournament of the benchmark field and the run's best, with Elo ratings as in Reproduction."""
    spec = importlib.util.spec_from_file_location("reproduction_report", HERE.parent / "Reproduction" / "report.py")
    reproduction = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(reproduction)
    field = load_field()
    bred = best_of(run, top)
    files = [WARRIORS / w["file"] for w in field["warriors"]] + bred
    folder = RUNS / run if run else BENCHMARK
    out = folder / "ranking.jsonl"
    seed = secrets.randbits(48)
    log_usage(
        f"breed.py rank {run or ''}".strip(),
        "rank the benchmark field" + (" with the run's best warriors" if run else ""),
        field_summary(field),
    )
    played = mars(
        "tournament", "--games", str(games), "--seed", str(seed), "--out", str(out), *device_args(gpu), *map(str, files)
    )
    if played.returncode != 0:
        raise SystemExit(f"mars tournament failed:\n{played.stderr[-2000:]}")
    pairs = reproduction.jsonl_pairs(out)
    out.unlink()

    normal = lambda s: re.sub(r"[^a-z0-9]", "", s.lower())
    published = {normal(p["name"]): p for p in field["published"]}
    names = {w["file"]: f"{w['name']} by {w['author']}" for w in field["warriors"]}
    listed = {w["file"]: published.get(normal(w["name"])) for w in field["warriors"]}
    standings = reproduction.standings([p.name for p in files], pairs)
    standings.sort(key=lambda s: -s["points"])  # the order of the published hill
    rows = []
    for n, s in enumerate(standings, 1):
        theirs = listed.get(s["program"])
        there = [theirs["rank"], f"{theirs['score']:.1f}"] if theirs else ["", ""]
        rows.append(
            [
                n,
                names.get(s["program"], f"**{s['program']}** (bred)"),
                f"{s['elo']:.0f}",
                f"{s['points']:.1f}",
                f"{s['score']:.1f}",
                *there,
            ]
        )
    ours = [s["points"] for s in standings if listed.get(s["program"])]
    theirs = [listed[s["program"]]["score"] for s in standings if listed.get(s["program"])]
    order = lambda values: [sorted(values, reverse=True).index(v) for v in values]
    n = len(ours)
    spearman = 1 - 6 * sum((a - b) ** 2 for a, b in zip(order(ours), order(theirs))) / (n * (n * n - 1))
    text = [
        "# Ranking against the benchmark field",
        "",
        f"Full tournament of {len(files)} warriors, {games} games per pair, master seed {seed}, `--standard pmars`,"
        f" played {datetime.now().date()}. Points are 3 per win and 1 per draw, per 100 games; Score is the percentage of"
        f" (wins + draws / 2). The published columns are the flat scores of the {field['source']} hill"
        f" ({field['ranking']}, pinned {field['pinned']}), which plays pMARS 0.9.4 with `-P`."
        f" Spearman correlation of our points with the published scores: {spearman:.2f}.",
        "",
        reproduction.md_table(["#", "Warrior", "Elo", "Points", "Score", "Published #", "Published"], rows),
    ]
    path = folder / ("validation.md" if run else "ranking.md")
    path.write_text("\n".join(text) + "\n")
    return path


def pmars_check(run: str | None, pairs: int, games: int) -> None:
    """Sample pairs on pMARS and on this engine. The placements differ, so the scores agree only
    within statistical error: |z| above 3 needs a look."""
    binary = os.environ.get("PMARS_092_BIN")
    if not binary:
        raise SystemExit("set PMARS_092_BIN to a pMARS 0.9.2 binary, see Modern/README.md")
    field = load_field()
    files = [WARRIORS / w["file"] for w in field["warriors"]]
    rng = random.Random(secrets.randbits(48))
    bred = best_of(run, 1)
    sample = (
        [(bred[0], o) for o in rng.sample(files, pairs)]
        if bred
        else [tuple(rng.sample(files, 2)) for _ in range(pairs)]
    )
    log_usage(
        f"breed.py pmars {run or ''}".strip(),
        "engine check against pMARS",
        ", ".join(sorted({p.name for pair in sample for p in pair if p in files})) + f" ({field['source']})",
    )
    out = RUNS / "pmars-check.jsonl"
    RUNS.mkdir(exist_ok=True)
    print(f"{'first':28}{'second':28}{'pMARS':>8}{'mars':>8}{'z':>7}")
    for first, second in sample:
        reference = subprocess.run(
            [binary, "-b", "-r", str(games), str(first), str(second)], capture_output=True, text=True
        )
        wins, losses, ties = map(int, re.search(r"Results: (\d+) (\d+) (\d+)", reference.stdout).groups())
        theirs = Tally([[wins, ties, losses, 0]])
        played = mars(
            "tournament",
            "--cpu",
            "--games",
            str(games),
            "--seed",
            str(rng.getrandbits(48)),
            "--out",
            str(out),
            str(first),
            str(second),
        )
        if played.returncode != 0:
            raise SystemExit(played.stderr[-2000:])
        r = json.loads(out.read_text())
        ours = Tally([[r["first_wins"], r["draws"], r["second_wins"], 0]])
        z = z_score(ours, theirs)
        print(
            f"{first.name:28}{second.name:28}{theirs.mean:8.3f}{ours.mean:8.3f}{z:7.2f}"
            + ("  CHECK" if abs(z) > 3 else "")
        )
    out.unlink()


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    commands = parser.add_subparsers(dest="command", required=True)
    gpu_help = "discrete GPUs to use, like 1 or 0,1, or cpu (default: every discrete GPU, also BREED_GPUS)"

    iterate = commands.add_parser("iterate", help="one inner run of the GA")
    iterate.add_argument("run")
    iterate.add_argument("--minutes", type=float, default=20, help="time budget of this inner run (20)")
    iterate.add_argument("--generations", type=int, help="stop after this many generations")
    iterate.add_argument("--gpu", help=gpu_help)
    created = iterate.add_argument_group("settings of a new run, kept in its config.json")
    for name in ("population", "screen_games", "refine_games", "hall_games", "hall_size", "opponents", "seed"):
        created.add_argument(f"--{name.replace('_', '-')}", type=int)
    created.add_argument("--generation-seconds", type=float)
    created.add_argument("--total-hours", type=float, help="time budget of the whole run, a stop rule")
    created.add_argument(
        "--no-seeds", dest="seeds", action="store_false", default=None, help="start from random programs only"
    )
    created.add_argument("--hu93-seeds", nargs="+", help="MARS.COM sources to start from as well")

    ranking = commands.add_parser("rank", help="full tournament of the benchmark field")
    ranking.add_argument("run", nargs="?")
    ranking.add_argument("--top", type=int, default=5, help="hall of fame members of the run to include (5)")
    ranking.add_argument("--games", type=int, default=1000)
    ranking.add_argument("--gpu", help=gpu_help)

    check = commands.add_parser("pmars", help="replay sample pairs on pMARS")
    check.add_argument("run", nargs="?")
    check.add_argument("--pairs", type=int, default=10)
    check.add_argument("--games", type=int, default=200)

    args = parser.parse_args()
    if args.command == "iterate":
        options = {k: v for k, v in vars(args).items() if k not in ("command", "run", "minutes", "generations", "gpu")}
        if options["hu93_seeds"]:
            options["hu93_seeds"] = [str(Path(p).resolve()) for p in options["hu93_seeds"]]
        print(Run(args.run, options, args.gpu).iterate(args.minutes, args.generations))
    elif args.command == "rank":
        print(rank(args.run, args.top, args.games, args.gpu))
    else:
        pmars_check(args.run, args.pairs, args.games)


if __name__ == "__main__":
    main()
