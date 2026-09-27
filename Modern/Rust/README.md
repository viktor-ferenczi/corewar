# MARS in Rust

A native reimplementation of the compiler and simulator of Viktor's 1993 `MARS.COM` (CoreWar MARS V1.0,
see [`Historical`](../../Historical)). With the same random seed a battle ends exactly as in `MARS.COM`,
down to the last process count in the statistics. It needs no DOSBox, and it plays the whole tournament
of [`Reproduction`](../../Reproduction) in about two minutes instead of four hours.

The command line is new; it does not take the `/X` options of `MARS.COM`.

## Build and run

Tested with Rust 1.90.

```bash
cargo build --release
```

Compile a program and list the result:

```bash
./target/release/mars compile ../../Historical/MICE.CWR
```

Play 500 wars and print the statistics the way `MARS /P=500` does:

```bash
./target/release/mars run --wars 500 ../../Historical/MICE.CWR ../../Historical/KILLER.CWR
```

Options of `run`: `--steps` (war length, 600000), `--queue` (processes per program, 64),
`--no-exec-other` (like `/E`), `--seed` (BIOS tick count, the time of day by default), `--log FILE`
(the binary statistics file of `/F`) and `--no-dat-test` (see below). `mars --help` lists them all.

Round robin of programs, every pair in both start orders, on all cores:

```bash
./target/release/mars tournament --out results --wars 500 --seed 1 ../../Historical/*.CWR
```

This writes `results/results/runs.csv` and one statistics file per run into `results/raw`. The
tournament of [`Reproduction`](Reproduction) was played this way.

## How exact it is

The code follows `MARS.ASM`, and a comparison of the assembled source with the binary showed that
`MARS.COM` was built from it without changes. The tests check the port against `MARS.COM` itself:

- `tests/golden/compile`: 37 test sources written to hit the odd corners of the compiler, and the
  39 historical programs. `MARS.COM` printed the error messages, and a copy patched with a small
  dumper (`tools/dump.asm`) wrote out the compiled code. Both must match exactly.
- `tests/golden/battle`: 168 battles recorded with copies of `MARS.COM` patched to a fixed random seed:
  all historical programs, queue lengths from 1 to 256, `/E`, three to five programs, very short and
  long wars, and a few programs written to hit edge cases (`tests/golden/progs`). The statistics
  text and the binary log must match exactly.
- `tests/compiler.rs`, `tests/engine.rs`, `tests/report.rs`, `tests/rng.rs`: unit tests of each rule
  and quirk below.

`cargo test` runs all of them in about a second. `tools/golden.py` records the reference data again. It
needs DOSBox 0.74 and nasm, and never modifies `MARS.COM`; the patched copies live in temporary folders.

## Quirks it reproduces

These come from `MARS.ASM` and all of them can change results.

Compiler:

- A word is a mnemonic only if it is exactly three letters. Everything else before the mnemonic is a
  label, so after a label even the words of a comment become labels (`LOOP ; jmp back` compiles a JMP).
- The source is read through a 16 character window. Labels count on their first 16 characters.
- Expressions are evaluated left to right in 16 bits, without precedence. Division takes the
  dividend as unsigned, so `-7/2` is 32764 (764 in the arena) and `-1/1` crashes `MARS.COM`.
- Numbers stop at 32769, a longer one is a syntax error.
- A label at instruction 59 leaves a `;` (59) in AL, so in the first pass the rest of its line counts as
  a comment.
- Whitespace after a label runs over line ends without counting them, and at the end of the file
  `MARS.COM` hangs. The port reports that as an error instead of hanging.
- A NUL byte ends the first pass. The second pass then reads the rest of the read buffer first, then the
  file again from its start.

Simulator:

- Operands are values of B fields. `<` decrements the B field of the pointer cell and marks that cell
  as written by the running program. A is read before B is decremented.
- Processes live in slots, not a queue. `SPL` takes the lowest free slot, and the scheduler goes round
  the slots from the one after the last executed.
- Programs are copied into memory without wrapping at the end of the arena. The part past cell 7999
  never runs, and when the start is past it, the program never runs but can not lose either.
- The memory after the arena is never cleared, not between wars and not between runs of `MARS.COM` in
  the same DOS session. Cells left there block later placements. `report::Session` carries this memory
  from run to run.
- In statistics mode `MARS.COM` checks every 16000 steps whether any DAT is left and ends the war as a
  draw if not. The check misses a DAT in cell 7999. `--no-dat-test` turns it off, which is how
  `MARS.COM` plays outside statistics mode.
- The step limit counts the steps of all programs together. A program without processes is skipped
  and does not use up a step.

## Source files

- `src/compiler.rs`: the Redcode compiler (`COMPILE`, `PASS1`, `PASS2`, `PARAMX`, `GETPARAM`)
- `src/engine.rs`: the simulator (`WAR`, `WAR1`, `LOADA`, `LOADB`, the instructions)
- `src/rng.rs`: the random generator used for placement (`RANDOMIZE`, `RANDOM`)
- `src/report.rs`: a whole `MARS.COM` run in statistics mode, its text output and binary log
- `src/tournament.rs`: parallel round robin
- `src/main.rs`: the command line
