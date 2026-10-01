# Researcher protocol

You are the researcher of a breeding run. `breed.py` is the lab: it runs a genetic algorithm and
GPU tournaments. You supply what random mutation rarely finds: a new strategy, a counter to a
specific opponent, a fix for a known loss. The GA tunes the constants. Read
[`README.md`](README.md) once for how the lab works.

The user starts you with a run name, for example `/loop follow Modern/Breeding/AGENT.md for run
night-1`. Work from the `Modern` folder.

## Rules

- Write your own code. Do not copy published warriors, in whole or in part, from memory or from
  any file. Never read `Breeding/benchmark/warriors/`. Knowing the classic strategies (stones,
  papers, scanners, clears, imps, vampires, quickscans and their mixes) and using them is fine;
  reproducing a named warrior is not.
- The benchmark warriors are opponents only. You see their names, authors and scores in the
  report and nothing else.
- Nothing gets submitted to a hill.
- Never read raw results (`gen/`, `*.jsonl`). The report has what you need.
- Only start `breed.py iterate` for the run you were given, with the time budget below, and one
  at a time. Don't pass `--gpu`: the script picks the discrete GPUs itself. No runs under
  `--standard hu93`.
- Don't edit `breed.py`, the benchmark files or other runs. If the lab looks broken, write down
  what you saw in the notebook and stop.

## Each iteration

1. Read the newest report in `Breeding/runs/RUN/reports/` and the last three entries of
   `Breeding/runs/RUN/NOTEBOOK.md`. On the first iteration there is no report yet: start from
   step 2 with your own opening ideas.
2. Pick one or two hypotheses. Write 5 to 20 candidates into `Breeding/runs/RUN/inbox/`, one
   `.red` file each. A candidate is a new warrior, or an edit of a bred one (the champion is in
   `champion.red`, the hall of fame in `hof/`). Each file needs an `;intent` line saying in one
   sentence what it tests. Check that they assemble:
   `./target/release/mars compile Breeding/runs/RUN/inbox/*.red`
3. Run `python3 Breeding/breed.py iterate RUN --minutes 20` and wait for it. It prints the path
   of the new report.
4. Append one entry to `NOTEBOOK.md`: the hypothesis, the candidates and why, what the report
   said about them (score, whether descendants did better), and what to try next. Keep it short
   and factual. The notebook is your memory across context compaction and restarts.
5. Check the last line of the report. If it says `STOP`, or the user's time budget is over, write
   a closing notebook entry with the main findings and end the loop. Otherwise continue with
   step 1.

## Rules of the game

`--standard pmars`: ICWS'94 as pMARS plays it, without P-space. Core 8000, at most 100
instructions, 80000 cycles per warrior, 8000 processes, minimum start distance 100. A war of two
warriors ends when one has no process left, or as a draw at the cycle limit.

## Syntax the assembler takes

- Opcodes: `DAT MOV ADD SUB MUL DIV MOD JMP JMZ JMN DJN SPL SLT CMP SEQ SNE NOP`. No `LDP`, `STP`
  or `PIN`.
- Modifiers: `.A .B .AB .BA .F .X .I`; without one the '94 default applies.
- Modes: `#` immediate, `$` direct, `*` and `@` A and B indirect, `{` and `<` with predecrement,
  `}` and `>` with postincrement.
- Labels, `EQU`, `FOR`/`ROF`, `ORG`, `END start`, expressions with `+ - * / %` and parentheses,
  and the constants `CORESIZE`, `MAXLENGTH`, `MAXPROCESSES`, `MAXCYCLES`, `MINDISTANCE`.
- Comments start with `;`. Begin a file like this:

```
;redcode-94nop
;name Short name
;author Claude
;intent One sentence: what this candidate tests.
;assert CORESIZE==8000
```

The lab rewrites every candidate in canonical form (numbers instead of labels) under an ID such as
`w000123`. Your annotated original is kept in `llm/iter-NNN/`, and the report lists the ID next to
your file name.

## Reading the report

- Leaderboard against the benchmark field: the comparable number across the whole run. Score is
  (wins + draws / 2) / games; 0.5 means level with the field.
- Leaderboard against the hall of fame: how the bred warriors do against each other.
- The champion's matchups, worst first: where to aim a counter.
- LLM candidates: whether each one assembled, its score, and the best score among its
  descendants. A weak candidate with strong descendants was a good idea badly tuned.
- Biggest improvements: what the GA changed, as diffs. Look for what it keeps rediscovering.
- Stagnation and diversity: few filled archive cells means the population has collapsed onto one
  strategy; propose different ones.
