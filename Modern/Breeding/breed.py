#!/usr/bin/env python3
"""Breed Redcode warriors: an LLM recombines each generation's winners, a remote MARS ranks them.

  breed.py step RUN     evaluate the generation on the remote MARS, keep the winners (top Elo),
                        and write one prompt per program of the next generation
  breed.py rank [RUN]   full tournament of the benchmark field, with the run's winners (local mars)
  breed.py pmars [RUN]  replay sample pairs on pMARS (PMARS_092_BIN) as an independent check

`step` needs no local engine: it talks to serve.py at MARS_URL with MARS_API_KEY. A coding agent
writes the programs the prompts ask for and calls `step` again, see AGENT.md. Warriors play under
the engine's default rules (--standard pmars: ICWS'94 without P-space, core 8000). `rank` and
`pmars` run on the machine that has the engine and the benchmark field, see benchmark/USAGE.md.
"""

import argparse
import base64
import hashlib
import importlib.util
import json
import math
import os
import random
import re
import secrets
import subprocess
import time
import urllib.error
import urllib.request
from collections import defaultdict
from datetime import datetime
from pathlib import Path

from benchmark.fetch import WARRIORS, field_summary, load_field, log_usage

HERE = Path(__file__).resolve().parent
MARS = Path(os.environ.get("MARS_BIN", HERE.parent / "target" / "release" / "mars"))
RUNS = HERE / "runs"
SEEDS = HERE / "seeds"
BENCHMARK = HERE / "benchmark"

DEFAULTS = {
    "population": 50,  # N: new programs per generation
    "keep": 0.2,  # the winners are this share of N, the best by Elo
    "min_parents": 2,  # programs a prompt combines, at least
    "max_parents": 0,  # at most; 0 for half of the winners, rounded up
    "games": 200,  # games per pair, half with each first mover
    "generations": 20,  # a stop rule
    "patience": 5,  # stop when the champion has not changed for this many generations
    "seeds": True,  # seeds/*.red play in the first generation
}
# Starting points for the first generation, which has no winners to combine yet.
FAMILIES = [
    "a stone: a small, fast bomber",
    "a paper: a replicator that spreads copies of itself",
    "a scanner: it looks for the opponent before it attacks",
    "a core clear: it wipes the whole core in a loop",
    "an imp spiral or imp ring, with whatever launches it",
    "a vampire: it bombs with jumps into a trap that wastes the opponent's processes",
    "a stone with imps as a backup",
    "a paper that also bombs",
    "a quickscan in front of another strategy",
    "a strategy of your own choice that is none of the usual ones",
]
OVER = ("done", "failed", "canceled")


# --- the local engine, for serve.py, rank and pmars ----------------------------------------------


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


def reproduction_report():
    """Reproduction/report.py as a module: Elo (Bradley-Terry) ratings, standings, tables."""
    spec = importlib.util.spec_from_file_location("reproduction_report", HERE.parent / "Reproduction" / "report.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


# --- the remote engine ---------------------------------------------------------------------------


def api(method: str, path: str, body: dict | None = None) -> dict:
    """One call to serve.py at MARS_URL. A refusal ends the script with the server's message."""
    url, key = os.environ.get("MARS_URL", "").rstrip("/"), os.environ.get("MARS_API_KEY", "")
    if not url or not key:
        raise SystemExit("set MARS_URL (like https://mars.example.org) and MARS_API_KEY")
    data = None if body is None else json.dumps(body).encode()
    request = urllib.request.Request(url + path, data=data, method=method)
    user = os.environ.get("MARS_API_USER")  # for a proxy that wants HTTP Basic
    basic = base64.b64encode(f"{user}:{key}".encode()).decode()
    request.add_header("Authorization", f"Basic {basic}" if user else f"Bearer {key}")
    for attempt in range(5):
        try:
            with urllib.request.urlopen(request, timeout=120) as response:
                return json.load(response)
        except urllib.error.HTTPError as error:
            try:
                reply = json.load(error)
            except ValueError:
                reply = {"error": error.reason}
            if error.code == 422 and "errors" in reply:
                return reply
            raise SystemExit(f"{method} {path}: {error.code} {reply.get('error')}")
        except OSError as error:  # the connection: wait and try again
            if attempt == 4:
                raise SystemExit(f"{method} {path}: {error}")
            time.sleep(2**attempt)


def tournament(programs: dict[str, str], games: int, seed: int, label: str) -> dict:
    """Play a full tournament on the remote MARS and wait for it. Picks up a tournament with the
    same label if this script was interrupted while one was queued or playing."""
    listed = api("GET", "/tournaments")["tournaments"]
    known = [t for t in listed if t["label"] == label and t["status"] != "canceled"]
    if known:
        job = api("GET", f"/tournaments/{known[-1]['id']}")
    else:
        job = api("POST", "/tournaments", {"programs": programs, "games": games, "seed": seed, "label": label})
    if "id" not in job:
        raise SystemExit(f"the server refused the tournament: {job['error']}")
    while job["status"] not in OVER:
        print(f"  tournament {job['id']}: {job['status']}, {job['progress']}% of {job['wars']} wars", flush=True)
        job = api("GET", f"/tournaments/{job['id']}?wait=30")
    if job["status"] != "done":
        raise SystemExit(f"tournament {job['id']} {job['status']}: {job.get('error', '')}")
    return job


# --- a run ---------------------------------------------------------------------------------------


def selection(rng: random.Random, rows: int, winners: list[str], least: int, most: int) -> list[dict]:
    """The random matrix: one row per new program, one 0 or 1 per winner. A row selects `least` to
    `most` winners, and `order` is the random order its prompt shows them in."""
    most = min(most or math.ceil(len(winners) / 2), len(winners))
    least = min(least, len(winners))
    matrix = []
    for _ in range(rows):
        order = rng.sample(winners, rng.randint(least, max(least, most)))
        matrix.append({"select": [int(w in order) for w in winners], "order": order})
    return matrix


class Run:
    def __init__(self, name: str, options: dict):
        self.name = name
        self.dir = RUNS / name
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
            if self.config["population"] + self.winner_count > 250:
                raise SystemExit("population and winners must stay below 250 programs, the limit of a full tournament")
            for folder in ("programs", "prompts", "reports", "generations"):
                (self.dir / folder).mkdir(parents=True, exist_ok=True)
            config_path.write_text(json.dumps(self.config, indent=1) + "\n")
        state_path = self.dir / "state.json"
        self.state = (
            json.loads(state_path.read_text())
            if state_path.exists()
            else {"generation": 0, "pending": [], "winners": [], "programs": {}, "history": []}
        )

    @property
    def winner_count(self) -> int:
        return max(1, round(self.config["keep"] * self.config["population"]))

    def save(self) -> None:
        (self.dir / "state.json").write_text(json.dumps(self.state, indent=1) + "\n")

    def path(self, program: str, kind: str = "programs") -> Path:
        return self.dir / kind / f"{program}.{'red' if kind == 'programs' else 'md'}"

    def shown(self, path: Path) -> str:
        """A path as the prompts and messages give it: relative to the Modern folder."""
        return str(path.relative_to(HERE.parent)) if path.is_relative_to(HERE.parent) else str(path)

    def plan(self, ranks: dict[str, str]) -> None:
        """Start the next generation: draw the matrix and write a prompt for each of its rows."""
        state, config = self.state, self.config
        state["generation"] += 1
        generation = state["generation"]
        rng = random.Random(f"{config['seed']}-{generation}")
        winners = state["winners"]
        matrix = selection(rng, config["population"], winners, config["min_parents"], config["max_parents"])
        state["pending"] = []
        for number, row in enumerate(matrix, 1):
            program = f"g{generation:03d}-r{number:03d}"
            row["program"] = program
            state["pending"].append(program)
            state["programs"][program] = {"generation": generation, "parents": row["order"]}
            family = None if winners else rng.choice(FAMILIES)
            self.path(program, "prompts").write_text(self.prompt(program, row["order"], ranks, family))
        plan = {"generation": generation, "winners": winners, "matrix": matrix}
        (self.dir / "generations" / f"{generation:03d}.json").write_text(json.dumps(plan, indent=1) + "\n")
        self.save()

    def prompt(self, program: str, parents: list[str], ranks: dict[str, str], family: str | None) -> str:
        target = self.shown(self.path(program))
        if len(parents) > 1:
            task = [
                f"Below are {len(parents)} warriors that were among the best of the last generation, in a random order.",
                "For each one, work out the ideas that make it work: how it attacks, how it survives, how it is laid",
                "out, which constants matter. Then write one new warrior that combines the ideas of all of them.",
                "",
                "Combining is not concatenating. Make the ideas work together in one program: share code where they",
                "overlap, keep the parts apart where they would hit each other, and retune the constants for the new",
                "layout. Drop what does not fit in 100 instructions. Prefer a warrior that works over one that has",
                "everything.",
            ]
        elif parents:
            task = [
                "Below is a warrior that was among the best of the last generation. Work out the ideas that make it",
                "work: how it attacks, how it survives, how it is laid out, which constants matter. Then write one new",
                "warrior that takes those ideas further. Do not hand back the same program with other constants.",
            ]
        else:
            task = [
                "There are no earlier warriors yet. Write an original warrior of your own.",
                f"Start from this kind of strategy: {family}.",
            ]
        text = [
            f"# Write the warrior {program}",
            "",
            *task,
            "",
            f"Write the warrior to `{target}` and nothing else. Check that it assembles if you have the means;",
            "`breed.py step` reports it if it does not.",
            "",
            "## Rules",
            "",
            "- ICWS'94 as pMARS plays it, without P-space. Core 8000, at most 100 instructions, 80000 cycles per",
            "  warrior, 8000 processes, minimum start distance 100. A war of two warriors ends when one has no",
            "  process left, or as a draw at the cycle limit. Warriors are ranked by a tournament of every pair.",
            "- Write your own code. Do not reproduce a published warrior, in whole or in part. Using the classic",
            "  techniques is fine.",
            "- Opcodes: `DAT MOV ADD SUB MUL DIV MOD JMP JMZ JMN DJN SPL SLT CMP SEQ SNE NOP`. No `LDP`, `STP`, `PIN`.",
            "- Modifiers: `.A .B .AB .BA .F .X .I`. Modes: `#` immediate, `$` direct, `*` `@` indirect by the A or B",
            "  field, `{` `<` with predecrement, `}` `>` with postincrement.",
            "- Labels, `EQU`, `FOR`/`ROF`, `ORG`, `END start`, expressions with `+ - * / %` and parentheses, and the",
            "  constants `CORESIZE`, `MAXLENGTH`, `MAXPROCESSES`, `MAXCYCLES`, `MINDISTANCE` are available.",
            "",
            "Begin the file like this"
            + (", and say in `;intent` which idea you took from which warrior:" if parents else ":"),
            "",
            "```",
            ";redcode-94nop",
            f";name {program}",
            ";author Claude",
            ";intent One or two sentences: "
            + ("the ideas combined and how they work together." if parents else "how it works."),
            ";assert CORESIZE==8000",
            "```",
        ]
        for number, parent in enumerate(parents, 1):
            text += [
                "",
                f"## Warrior {number}: {parent} ({ranks[parent]})",
                "",
                "```",
                self.path(parent).read_text().rstrip(),
                "```",
            ]
        return "\n".join(text) + "\n"

    def step(self, skip_broken: bool) -> int:
        """Advance the run as far as it can go without the LLM. Returns the exit code."""
        state, config = self.state, self.config
        if state["generation"] == 0:
            if config["seeds"]:
                for seed in sorted(SEEDS.glob("*.red")):
                    self.path(f"seed-{seed.stem}").write_text(seed.read_text())
                    state["programs"][f"seed-{seed.stem}"] = {"generation": 0, "parents": []}
            self.plan({})
            return self.waiting()
        if not state["pending"]:
            print(self.stop_rule())
            return 0
        missing = [p for p in state["pending"] if not self.path(p).exists()]
        if missing:
            print(f"{len(missing)} programs of generation {state['generation']} are not written yet.")
            return self.waiting(missing)

        seeds = (
            [p for p, meta in state["programs"].items() if meta["generation"] == 0] if state["generation"] == 1 else []
        )
        members = state["pending"] + state["winners"] + seeds
        sources = {p: self.path(p).read_text() for p in members}
        compiled = api("POST", "/compile", {"programs": {p: sources[p] for p in state["pending"]}})["programs"]
        broken = {p: result["errors"] for p, result in compiled.items() if not result["ok"]}
        if broken and not skip_broken:
            print(f"{len(broken)} programs do not assemble. Fix them, then run step again:")
            for program, errors in broken.items():
                print(f"  {self.shown(self.path(program))}: {'; '.join(errors)}")
            return 1
        members = [p for p in members if p not in broken]
        sources = {p: sources[p] for p in members}

        generation = state["generation"]
        digest = hashlib.sha256(json.dumps(sources, sort_keys=True).encode()).hexdigest()[:12]
        seed = random.Random(f"{config['seed']}-{generation}-play").getrandbits(48)
        print(f"generation {generation}: {len(members)} programs, {config['games']} games per pair", flush=True)
        job = tournament(sources, config["games"], seed, f"{self.name} generation {generation} {digest}")

        pairs = defaultdict(lambda: {"games": 0, "wins": 0, "losses": 0, "draws": 0})
        for run in job["runs"]:
            for me, other in (("first", "second"), ("second", "first")):
                pair = pairs[run[me], run[other]]
                pair["games"] += run["games"]
                pair["wins"] += run[f"{me}_wins"]
                pair["losses"] += run[f"{other}_wins"]
                pair["draws"] += run["draws"]
        reproduction = reproduction_report()
        standings = reproduction.standings(members, pairs)  # best Elo first
        winners = [row["program"] for row in standings[: self.winner_count]]
        champion = standings[0]["program"]
        state["history"].append(
            {
                "generation": generation,
                "champion": champion,
                "elo": round(standings[0]["elo"]),
                "new_winners": len(set(winners) - set(state["winners"])),
                "broken": sorted(broken),
            }
        )

        record_path = self.dir / "generations" / f"{generation:03d}.json"
        record = json.loads(record_path.read_text())
        record |= {
            "tournament": {k: job[k] for k in ("id", "seed", "games", "wars", "seconds")},
            "broken": broken,
            "standings": standings,
        }
        record_path.write_text(json.dumps(record, indent=1) + "\n")
        self.report(generation, standings, winners, broken, job, reproduction)
        (self.dir / "champion.red").write_text(sources[champion])

        state["winners"], state["pending"] = winners, []
        ranks = {
            row["program"]: f"rank {n} of {len(standings)}, Elo {row['elo']:.0f}" for n, row in enumerate(standings, 1)
        }
        print(
            f"generation {generation} played: champion {champion}, report {self.shown(self.dir / 'reports' / f'{generation:03d}.md')}"
        )
        stop = self.stop_rule()
        if stop.startswith("STOP"):
            self.save()
            print(stop)
            return 0
        self.plan(ranks)
        return self.waiting()

    def waiting(self, programs: list[str] | None = None) -> int:
        """Tell the agent which prompts to answer."""
        programs = programs or self.state["pending"]
        print(f"Write these {len(programs)} programs, each from its prompt, then run step again:")
        for program in programs:
            print(f"  {self.shown(self.path(program, 'prompts'))} -> {self.shown(self.path(program))}")
        return 0

    def stop_rule(self) -> str:
        history, config = self.state["history"], self.config
        if len(history) >= config["generations"]:
            return f"STOP: {config['generations']} generations played. The champion is {history[-1]['champion']}."
        recent = [h["champion"] for h in history[-config["patience"] :]]
        if (
            len(history) > config["patience"]
            and len(set(recent)) == 1
            and history[-config["patience"] - 1]["champion"] == recent[0]
        ):
            return f"STOP: the champion {recent[0]} has not changed for {config['patience']} generations."
        return "Continue: no stop rule has fired."

    def report(
        self, generation: int, standings: list[dict], winners: list[str], broken: dict, job: dict, reproduction
    ) -> None:
        programs = self.state["programs"]
        rows = []
        for n, row in enumerate(standings, 1):
            meta = programs[row["program"]]
            born = (
                "seed"
                if meta["generation"] == 0
                else "new" if meta["generation"] == generation else f"gen {meta['generation']}"
            )
            rows.append(
                [
                    n,
                    row["program"],
                    f"{row['elo']:.0f}",
                    f"{row['score']:.1f}",
                    f"{row['wins']} / {row['draws']} / {row['losses']}",
                    born,
                    "kept" if row["program"] in winners else "",
                    " ".join(meta["parents"]),
                ]
            )
        history = ", ".join(f"{h['generation']}: {h['champion']}" for h in self.state["history"][-8:])
        text = [
            f"# Run {self.name}, generation {generation}",
            "",
            f"Full tournament of {len(standings)} programs, {job['games']} games per pair, {job['wars']} wars in"
            f" {job['seconds']} s, master seed {job['seed']}. Elo is a Bradley-Terry rating with mean 1500. Score is the"
            f" percentage of (wins + draws / 2). The best {len(winners)} by Elo are kept: they are the parents of the next"
            " generation and play in it again.",
            "",
            reproduction.md_table(["#", "Program", "Elo", "Score", "W / D / L", "Born", "", "Parents"], rows),
            "",
            f"- New among the winners: {self.state['history'][-1]['new_winners']} of {len(winners)}.",
            f"- Champion by generation: {history}.",
        ]
        if broken:
            text.append(f"- Left out because they did not assemble: {', '.join(sorted(broken))}.")
        text.append(f"- {self.stop_rule()}")
        (self.dir / "reports" / f"{generation:03d}.md").write_text("\n".join(text) + "\n")


# --- ranking and the pMARS check -----------------------------------------------------------------


def best_of(run: str | None, top: int) -> list[Path]:
    """The run's winners, best first."""
    if not run:
        return []
    winners = json.loads((RUNS / run / "state.json").read_text())["winners"]
    return [RUNS / run / "programs" / f"{program}.red" for program in winners[:top]]


def rank(run: str | None, top: int, games: int, gpu: str | None) -> Path:
    """Full tournament of the benchmark field and the run's best, with Elo ratings as in Reproduction."""
    reproduction = reproduction_report()
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

    step = commands.add_parser("step", help="evaluate the generation and write the prompts of the next")
    step.add_argument("run")
    step.add_argument("--skip-broken", action="store_true", help="play without the programs that do not assemble")
    created = step.add_argument_group("settings of a new run, kept in its config.json")
    for name in ("population", "min_parents", "max_parents", "games", "generations", "patience", "seed"):
        created.add_argument(f"--{name.replace('_', '-')}", type=int)
    created.add_argument("--keep", type=float, help="share of the population kept as winners (0.2)")
    created.add_argument("--no-seeds", dest="seeds", action="store_false", default=None, help="leave out seeds/*.red")

    ranking = commands.add_parser("rank", help="full tournament of the benchmark field")
    ranking.add_argument("run", nargs="?")
    ranking.add_argument("--top", type=int, default=5, help="winners of the run to include (5)")
    ranking.add_argument("--games", type=int, default=1000)
    ranking.add_argument("--gpu", help=gpu_help)

    check = commands.add_parser("pmars", help="replay sample pairs on pMARS")
    check.add_argument("run", nargs="?")
    check.add_argument("--pairs", type=int, default=10)
    check.add_argument("--games", type=int, default=200)

    args = parser.parse_args()
    if args.command == "step":
        options = {k: v for k, v in vars(args).items() if k not in ("command", "run", "skip_broken")}
        raise SystemExit(Run(args.run, options).step(args.skip_broken))
    elif args.command == "rank":
        print(rank(args.run, args.top, args.games, args.gpu))
    else:
        pmars_check(args.run, args.pairs, args.games)


if __name__ == "__main__":
    main()
