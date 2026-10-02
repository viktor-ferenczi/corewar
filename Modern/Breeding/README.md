# Breeding warriors

A loop that breeds Redcode warriors under the engine's default rules (`--standard pmars`: ICWS'94
as pMARS plays it, without P-space, core 8000, length 100, 80000 cycles, 8000 processes, minimum
distance 100). These are the rules of the KOTH '94 no-P-space hills, so the results can be
compared with published warriors.

An LLM does the recombination and a tournament does the selection:

- [`breed.py step`](breed.py) is the bookkeeping. It has each generation played as a full
  tournament, keeps the best 20% by Elo, draws a random selection matrix over those winners, and
  writes one prompt per program of the next generation.
- A coding agent answers the prompts: each asks for one new warrior that combines the ideas of
  the winners its matrix row selects. [`AGENT.md`](AGENT.md) is the agent's prompt.
- [`serve.py`](serve.py) plays the tournaments. It runs on a machine with GPUs, and `breed.py`
  reaches it over HTTP, so the loop itself can run anywhere, for example in a cloud agent
  session. See [SERVING.md](SERVING.md).

Everything stays local to these two machines. Nothing here submits a warrior to a hill.

## Setup

```bash
cargo build --release              # in Modern, on the machine that plays: builds target/release/mars
python3 Breeding/benchmark/fetch.py   # optional, there too: the benchmark field for `breed.py rank`
```

The scripts need Python 3.11 or later and nothing outside the standard library.

## Running it

On the machine with the GPUs, start the server (see [SERVING.md](SERVING.md)):

```bash
MARS_API_KEY=... python3 Breeding/serve.py --port 8080
```

Where the loop runs, only Python and this repository are needed, no engine:

```bash
export MARS_URL=https://mars.example.org MARS_API_KEY=...
python3 Breeding/breed.py step my-run --population 50 --generations 20
```

Then start a coding agent with a prompt such as "Follow `Modern/Breeding/AGENT.md` for run
`my-run`". It calls `step`, writes the programs the prompts ask for, and repeats until `step`
prints `STOP`.

| Setting | Default | |
|:--|:--|:--|
| `--population` | 50 | N: new programs per generation |
| `--keep` | 0.2 | the winners are this share of N, so M = round(0.2 N) |
| `--min-parents` | 2 | winners a row selects, at least |
| `--max-parents` | half of M, rounded up | at most |
| `--games` | 200 | games per pair, half with each first mover |
| `--generations` | 20 | stop after this many |
| `--patience` | 5 | stop when the champion has not changed for this many generations |
| `--seed` | random | the run's random seed |
| `--no-seeds` | | leave [`seeds/*.red`](seeds) out of the first generation |

The settings count only when the run is created and are kept in its `config.json`.
`MARS_API_USER` makes `breed.py` send the key as the password of HTTP Basic instead of as a
bearer token, for a reverse proxy that wants Basic.

## How it works

One generation, as `step` sees it:

1. **Validate.** Every program the prompts asked for must be written, and `step` sends them to
   the server's `/compile` before anything plays. A program that does not assemble is replaced,
   not repaired: it is moved to `rejected/`, its matrix row is drawn again (a new random
   selection of winners, seeded), and a new prompt asks for a new program. `step` stops there
   until the replacements are written, and validates those the same way.
2. **Play.** The N new programs and the M winners of the generation before play a full
   tournament on the server. In the first generation the seeds play too. The tournament's master
   seed is derived from the run's seed and the generation.
3. **Rank.** Elo ratings (Bradley-Terry, as in [`Reproduction`](../Reproduction)) from all pairs.
   The best M = round(0.2 N) are the new winners. Old winners compete for these places like
   everyone else, so the best warrior is kept until something beats it.
4. **Draw the matrix.** N rows and M columns of 0 and 1. Each row selects 2 to ceil(M / 2)
   winners at random, in a random order. The matrix is drawn from the run's seed and the
   generation number, so the same run draws the same matrix again.
5. **Write the prompts.** One per row: the selected winners' sources in the row's order, with
   their rank and Elo, and the instruction to work out each one's ideas and write one new warrior
   that combines them. The prompt carries the rules and the syntax, so it can be answered without
   any other context.

The first generation has no winners yet. Its prompts ask for an original warrior and each
suggests a kind of strategy, drawn at random, so the starting population is varied.

All variation comes from the LLM, steered by the matrix: which ideas meet is random, how they are
combined is the LLM's work. Elo within the generation is a relative measure. To see how the
winners do against published warriors, use `breed.py rank` on the machine that has the benchmark
field.

### Files of a run

| Path | Content |
|:--|:--|
| `config.json` | the settings and the random seed |
| `state.json` | the current generation, the programs still to write, the winners, the history |
| `prompts/gNNN-rNNN.md` | one prompt per matrix row |
| `programs/gNNN-rNNN.red` | the program written for that prompt; `seed-*.red` are the seeds |
| `generations/NNN.json` | the winners going in, the matrix, the replaced programs with their errors, and after the tournament the standings |
| `rejected/` | programs that did not assemble and were replaced |
| `reports/NNN.md` | the generation's ranking with Elo, and which programs were kept |
| `champion.red` | the best program of the last tournament |

A run is small text throughout and is meant to be committed.

## The benchmark field

The yardstick for bred warriors is the 59 warriors of the
[Koenigstuhl 94nop Top-50](https://asdflkj.net/COREWAR/koenigstuhl.html) hill kept by Christoph
Birk. They are other people's work, so:

- They are never committed. [`benchmark/field.json`](benchmark/field.json) pins each one by file,
  name, author and SHA-256, along with the source's terms as checked. `fetch.py` refuses an
  archive that no longer matches; `fetch.py --repin` updates the pins after you have checked the
  terms again.
- They are opponents only, and only on the machine that downloaded them. The breeding loop never
  sees them: no prompt contains one, and the server does not serve them. The agent is told not
  to copy published warriors.
- The ranking names each opponent with its author.
- [`benchmark/USAGE.md`](benchmark/USAGE.md) records every download and use with its date, run
  and purpose. `fetch.py`, `breed.py rank` and `breed.py pmars` write the entries.

[`benchmark/ranking.md`](benchmark/ranking.md) is the field played against itself by this engine,
next to the hill's published scores (`breed.py rank`).

`breed.py rank my-run` plays the run's best winners in a full tournament with the benchmark field
and writes `runs/my-run/validation.md`, with Elo ratings. `breed.py pmars my-run` replays sample
pairs on pMARS 0.9.2 (`PMARS_092_BIN`, see the [engine's README](../README.md#how-exact-it-is)) as
an independent check. Placements differ between the engines, so the scores agree only within
statistical error. Both need the local `mars` binary. They play on every discrete GPU that
`mars gpus` lists, or on the CPU when there is none, never on an integrated or software GPU;
`--gpu 1`, `--gpu 0,1`, `--gpu cpu` or `BREED_GPUS` narrow that down.

## Tests

```bash
python3 -m unittest discover -s Breeding
```

They take a few seconds. The breeding loop is tested against a `serve.py` on a local port, with a
stand-in for the LLM, and the ranking against a stand-in field made of the seed warriors, so they
need neither the download nor a GPU. `MARS_TEST_GPUS=1` pins them to that adapter, like
the engine's GPU tests; `MARS_BIN` points them at another build.
