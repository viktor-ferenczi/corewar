# Breeding warriors

An auto-research loop that breeds Redcode warriors under the engine's default rules
(`--standard pmars`: ICWS'94 as pMARS plays it, without P-space, core 8000, length 100, 80000
cycles, 8000 processes, minimum distance 100). These are the rules of the KOTH '94 no-P-space
hills, so the results can be compared with published warriors.

A genetic algorithm proposes warriors, and gauntlet tournaments of the [Rust MARS](../README.md)
on the GPUs decide which ones survive. A Claude Code conversation can sit on top of it as the
researcher: it reads the report of each inner run, writes new candidates into an inbox and keeps a
lab notebook. [`AGENT.md`](AGENT.md) is its protocol.

Everything stays local. Nothing here submits a warrior to a hill.

## Setup

```bash
cargo build --release              # in Modern, builds target/release/mars
python3 Breeding/benchmark/fetch.py
```

`fetch.py` downloads the benchmark field into `benchmark/warriors/`, which Git ignores. The
scripts need Python 3.11 or later and nothing outside the standard library.

## The benchmark field

The opponents are the 59 warriors of the
[Koenigstuhl 94nop Top-50](https://asdflkj.net/COREWAR/koenigstuhl.html) hill kept by Christoph
Birk. They are other people's work, so:

- They are never committed. [`benchmark/field.json`](benchmark/field.json) pins each one by file,
  name, author and SHA-256, along with the source's terms as checked. `fetch.py` refuses an
  archive that no longer matches; `fetch.py --repin` updates the pins after you have checked the
  terms again.
- They are opponents only. The GA never takes one as a parent, so no bred warrior holds their
  code, and the researcher is told not to copy published warriors.
- Reports name each opponent with its author.
- [`benchmark/USAGE.md`](benchmark/USAGE.md) records every download and use with its date, run
  and purpose. `fetch.py` and `breed.py` write the entries.

[`benchmark/ranking.md`](benchmark/ranking.md) is the field played against itself by this engine,
next to the hill's published scores (`breed.py rank`).

## Running it

```bash
python3 Breeding/breed.py iterate my-run --minutes 20
```

This creates `runs/my-run` on first use, breeds for 20 minutes and prints the path of the report.
Calling it again continues the run. The options that shape a run (`--population`,
`--screen-games`, `--refine-games`, `--hall-games`, `--hall-size`, `--generation-seconds`,
`--opponents`, `--seed`, `--total-hours`, `--no-seeds`, `--hu93-seeds`) count only when the run is
created, and are kept in its `config.json`. `--minutes` and `--generations` limit one inner run:
it ends when either is reached.

A GA-only run is the same command with nobody filling the inbox. It is the control the researcher
runs are measured against.

`breed.py rank my-run` plays the run's best hall of fame members in a full tournament with the
benchmark field and writes `runs/my-run/validation.md`, with Elo ratings as in
[`Reproduction`](../Reproduction). `breed.py pmars my-run` replays sample pairs on pMARS 0.9.2
(`PMARS_092_BIN`, see the [engine's README](../README.md#how-exact-it-is)) as an independent check.
Placements differ between the engines, so the scores agree only within statistical error.

### GPUs

`breed.py` plays on every discrete GPU that `mars gpus` lists, and on the CPU when there is none.
It never uses an integrated GPU or a software one such as llvmpipe. `--gpu 1`, `--gpu 0,1` or
`--gpu cpu` (or the `BREED_GPUS` variable) narrow that down, to discrete GPUs only.

The first screen of an inner run measures the wars per second. If a generation would take longer
than `generation_seconds` (120), the games per pair of that inner run are cut to fit, and the
report says so.

### From a remote agent

[`serve.py`](serve.py) is a REST API over the `mars` binary, with a pre-shared key. A coding agent
elsewhere can drive its own search with it: it sends program variants and gets tournament or
gauntlet results back. See [SERVING.md](SERVING.md).

## How it works

Each generation:

1. The archive's warriors are mutated and crossed into `population` new candidates. In the first
   generation of an inner run the inbox's candidates join them, and in the first generation of a
   run the seeds do: [`seeds/*.red`](seeds), optionally 1993 `MARS.COM` sources (`--hu93-seeds`),
   and random programs.
2. Screen: every candidate and every archive member plays every opponent, 40 games per pair.
3. Refine: the top 20% play 400 games per pair.
4. A refined newcomer enters the archive if its cell is empty or it beats the holder.
5. The generation's best newcomer plays the benchmark field with 2000 games per pair. While the
   hall of fame has room it joins if it beats the best member, after that it replaces the weakest
   member if it beats that one, in both cases with z > 2.

The opponents are the benchmark field and the hall of fame (at most 20 bred warriors), which keeps
the population from specializing against a fixed field. Every tournament gets a new master seed,
recorded in `seeds.txt` next to the results, so the GA can't learn the placements.

Fitness is the mean score, (wins + draws / 2) / games. Reports also give the KOTH score, 3 points
per win and 1 per draw, per 100 games.

The archive is a small MAP-Elites grid over two descriptors: the program length and the mean
number of processes left at the end of a war. Each cell keeps its best warrior, so papers don't
push out every stone for being a little ahead early on.

The operators work on the assembled instructions: point mutation of an opcode, modifier, mode or
value; insert, delete or duplicate an instruction; one-point and two-point crossover; and a
constant sweep, which tries eight values of one constant at once, preferably the step of an `ADD`
or `SUB`. New values are drawn near the old one, as a small offset, or as a step coprime with
8000.

Every candidate is written out in canonical form from its assembled instructions: explicit
modifiers, numeric addresses, no labels, `ORG` for the start. That file is the one that plays, and
it runs on pMARS unchanged. A candidate that doesn't assemble, or is longer than 100 instructions,
is rejected before any war, and the error goes into the report.

## Files of a run

| Path | Content | In Git |
|:--|:--|:--|
| `config.json` | the run's settings and its random seed | yes |
| `state.json` | the archive, the hall of fame, the champion's score per iteration | yes |
| `inbox/` | candidates for the next inner run, one `.red` file each with an `;intent` line | |
| `llm/iter-NNN/` | the researcher's annotated originals, after they were taken | yes |
| `hof/hof-ID.red` | the hall of fame, canonical sources | yes |
| `champion.red` | the best warrior against the benchmark field | yes |
| `reports/iter-NNN.md` | the report of each inner run | yes |
| `NOTEBOOK.md` | the researcher's lab notebook | yes |
| `validation.md` | the ranking written by `breed.py rank` | yes |
| `lineage.jsonl` | every candidate: parents, operator, intent, scores | no |
| `gen/NNNN/` | the candidates, compressed raw results and seeds of a generation | no |

## Tests

```bash
python3 -m unittest discover -s Breeding
```

They take a few seconds, and cover `serve.py` too. The ones that play wars use a stand-in field made of the seed warriors,
so they need neither the download nor a GPU. `MARS_TEST_GPUS=1` pins them to that adapter, like
the engine's GPU tests; `MARS_BIN` points them at another build.
