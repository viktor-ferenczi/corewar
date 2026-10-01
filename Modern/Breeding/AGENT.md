# Breeding loop

You run a breeding loop for Core War warriors. Your goal: breed the strongest warrior you can
within the run's generation limit, by repeating one cycle until the script says `STOP`.

The script `Breeding/breed.py` does the bookkeeping. Each generation it has the programs played
against each other on a remote MARS, keeps the best 20% by Elo rating as winners, and draws a
random matrix: one row per program of the next generation, selecting a few of the winners. For
each row it writes a prompt that asks for one new warrior combining the ideas of the selected
winners. You are the one who answers those prompts. The winners play again in the next
generation, so a good warrior is never lost.

Work from the `Modern` folder. The run's name comes from the user, here `RUN`. The environment
must have `MARS_URL` and `MARS_API_KEY` (and `MARS_API_USER` if the proxy wants HTTP Basic); if
`breed.py` says they are missing or the server refuses the key, stop and tell the user.

## The cycle

1. Run `python3 Breeding/breed.py step RUN`. On the first call, pass the settings the user gave
   (for example `--population 50 --generations 20`); later calls take them from the run.
2. The output lists prompt files and the program file each one asks for:
   `Breeding/runs/RUN/prompts/g003-r017.md -> Breeding/runs/RUN/programs/g003-r017.red`
3. Answer every prompt. Give each one to a subagent with a fresh context, several at a time, with
   an instruction like: "Read `PROMPT FILE` and do what it says." The prompt file has everything
   the subagent needs. Without subagents, answer them yourself one by one, and treat each on its
   own: read only that prompt, write only that program.
4. Run `step` again. It checks that every program is there and assembles.
   - If it lists missing programs, write them.
   - If it lists programs that do not assemble, fix each one from its error message while keeping
     to its prompt, and run `step` again. If one cannot be fixed after two tries, run
     `step --skip-broken` and it sits this generation out.
   - Otherwise it plays the tournament (it prints the progress while it waits), writes the report
     `Breeding/runs/RUN/reports/NNN.md`, and lists the prompts of the next generation.
5. Commit the run folder `Breeding/runs/RUN` with a message such as `RUN generation 3`. Push only
   if the user asked for that.
6. If the output says `STOP`, go to "When it stops". Otherwise continue with step 3.

## Rules

- The programs must be new code written for their prompt. Do not copy a published warrior, in
  whole or in part, from memory or from any file. Classic techniques are fine.
- Do not write programs outside the prompts, do not edit a program after its tournament, and do
  not edit `state.json`, `generations/`, the prompts, `breed.py` or another run. The matrix decides
  which winners are combined; do not pick parents yourself.
- Do not read `Breeding/benchmark/warriors/` if it exists.
- One `step` at a time. If a `step` was interrupted while a tournament was playing, run it again:
  it finds the tournament on the server and waits for it.
- Nothing gets submitted to a hill.
- If the script fails in a way these instructions do not cover, stop and report what it printed.

## When it stops

`step` prints `STOP` when the generation limit is reached or the champion has not changed for
several generations. Then:

1. Read the last report and `Breeding/runs/RUN/champion.red`.
2. Write `Breeding/runs/RUN/SUMMARY.md`: the champion and how it works, which ideas survived
   across the generations (the reports list each winner's parents), and what did not work.
3. Commit, and tell the user where the champion and the summary are.

## Files of a run

| Path | Content |
|:--|:--|
| `config.json` | the settings and the random seed of the run |
| `state.json` | the current generation, the programs still to write, the winners, the history |
| `prompts/gNNN-rNNN.md` | one prompt per matrix row |
| `programs/gNNN-rNNN.red` | the program written for that prompt; `seed-*.red` are the starting seeds |
| `generations/NNN.json` | the winners going in, the matrix, and after the tournament the standings |
| `reports/NNN.md` | the generation's ranking with Elo, and which programs were kept |
| `champion.red` | the best program of the last tournament |
