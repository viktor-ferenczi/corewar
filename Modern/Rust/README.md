# MARS in Rust

A native reimplementation of the compiler and simulator of Viktor's 1993 `MARS.COM` (CoreWar MARS V1.0,
see [`Historical`](../../Historical)). With the same random seed a battle ends exactly as in `MARS.COM`,
down to the last process count in the statistics. It needs no DOSBox, and it plays the whole tournament
of [`Reproduction`](../../Reproduction) in about two minutes on the CPU instead of four hours, or in 22
seconds on two GPUs.

Where it differs from `MARS.COM` on purpose:

- The command line is new; it does not take the `/X` options of `MARS.COM`.
- The output has the same format, except for the first line, `CoreWar MARS Rust V1.0 - Viktor Ferenczi
  2026`, so you can tell which one produced it.
- `MARS.COM` ends a line only at a CR and reads an LF as a space, so a file with Unix line ends is one
  long comment for it. The port takes an LF without a CR before it as a line end too, so sources work
  with CR LF, LF and CR line ends alike. Files with DOS line ends compile exactly as in `MARS.COM`.
- Where `MARS.COM` would hang or crash while compiling, the port stops with an error.

## Build and run

Tested with Rust 1.90. The GPU support (cargo feature `gpu`, on by default) uses
[wgpu](https://wgpu.rs), which runs on Vulkan on Linux, with NVIDIA and AMD GPUs alike.

```bash
cargo build --release
```

Without GPU support the binary is a tenth of the size and needs no Vulkan:

```bash
cargo build --release --no-default-features
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

## Tournaments

A tournament plays every pair of 2 to 256 programs in both start orders, 1000 games per pair by
default, on all CPU cores:

```bash
./target/release/mars tournament --out results.jsonl --seed 1 ../../Historical/*.CWR
```

Every pair plays two MARS runs, one per start order, with random seeds derived from the master seed
`--seed`, so a tournament can be repeated exactly. Both runs of a pair share a DOS session like in
`Reproduction/mars.py`. The file gets one JSON object per run and line, in pair order, like this one
from [`Reproduction/results/runs.jsonl`](Reproduction/results/runs.jsonl):

```json
{"first":"ANTIIMP","second":"ARTUR-1","seed":1276916,"games":500,"first_wins":0,"second_wins":73,"draws":427,"first_pcs":570,"second_pcs":30236,"steps":272061937,"max_steps":600000,"queue":64,"exec_other":true}
```

`first_pcs` and `second_pcs` are the processes left at the end of the wars, summed. `--format sta`
writes the old layout instead: `results/runs.csv` and the statistics text of every run in `raw/`,
into the folder given by `--out`. `--games`, `--steps`, `--queue` and `--no-exec-other` work like
for `run`.

### On GPUs

`--gpu` plays the wars on the first GPU, `--gpu 0,1` on the listed ones, `--gpu all` on all GPUs of
the best kind present (all discrete GPUs if there are any: a much slower integrated GPU would hold up
the end of a tournament with its last wars). `mars gpus` lists them:

```
0  NVIDIA GeForce RTX 4090  (DiscreteGpu, Vulkan, NVIDIA)
1  NVIDIA GeForce RTX 4090  (DiscreteGpu, Vulkan, NVIDIA)
2  AMD Ryzen 7 9800X3D 8-Core Processor (RADV RAPHAEL_MENDOCINO)  (IntegratedGpu, Vulkan, radv)
3  llvmpipe (LLVM 20.1.2, 256 bits)  (Cpu, Vulkan, llvmpipe)
```

The results are exactly the same as on the CPU, war by war. This works because placement is the only
part of a MARS run that carries from war to war (the random generator and the memory after the
arena), and it does not depend on how the wars end. So the CPU works out the positions of all wars
first, the way MARS would, and then every war is fought on its own. On a GPU every thread plays one
war at a time in its own arena (`src/gpu.wgsl`, a port of `Engine::fight`) and takes the next war
when its war ends. Short dispatches keep clear of the driver watchdog, and the GPUs take chunks of
wars from a shared queue, so a faster GPU plays more of them. See [Performance](#performance) for
timings.

## How exact it is

The code follows `MARS.ASM`, and a comparison of the assembled source with the binary showed that
`MARS.COM` was built from it without changes. The tests check the port against `MARS.COM` itself:

- `tests/golden/compile`: 36 test sources written to hit the odd corners of the compiler, and the
  39 historical programs. `MARS.COM` printed the error messages, and a copy patched with a small
  dumper (`tools/dump.asm`) wrote out the compiled code. Both must match exactly.
- `tests/golden/battle`: 168 battles recorded with copies of `MARS.COM` patched to a fixed random seed:
  all historical programs, queue lengths from 1 to 256, `/E`, three to five programs, very short and
  long wars, and a few programs written to hit edge cases (`tests/golden/progs`). The statistics
  text and the binary log must match exactly.
- `tests/compiler.rs`, `tests/engine.rs`, `tests/report.rs`, `tests/rng.rs`: unit tests of each rule
  and quirk below.
- `tests/tournament.rs`: a tournament gives the same results as playing its MARS runs one by one.
- `tests/gpu.rs`: the GPU against the CPU engine, war by war, with various settings, on every adapter
  found, llvmpipe included. Without any adapter there is nothing to compare.

`cargo test --release` runs all of them in about 15 seconds. `tools/golden.py` records the reference
data again. It needs DOSBox 0.74 and nasm, and never modifies `MARS.COM`; the patched copies live in
temporary folders. It gives DOSBox the sources with CR LF line ends, whatever the checkout has.

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
  `MARS.COM` hangs.
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
- `src/tournament.rs`: tournaments, placement planning, fighting on CPU threads, JSONL and `.sta` output
- `src/gpu.rs`, `src/gpu.wgsl`: fighting on GPUs
- `src/main.rs`: the command line

## Performance

Measured on two machines, with the same binary, built on the first one with rustc 1.90.0 and wgpu
30.0.1. Every run below gave exactly the same results as the CPU.

- Workstation: AMD Ryzen 7 9800X3D (8 cores, 16 threads), two NVIDIA GeForce RTX 4090 (24 GB, driver
  595.91.07), Linux Mint 22.3, kernel 7.0.0.
- polaris: two AMD Radeon AI PRO R9700 (Navi 48, RDNA 4, Mesa 25.0.7 RADV), Debian 13, kernel 7.0.12.
  Its CPU (Ryzen 9 5950X) was busy with other work, so only its GPUs were measured.

The tournament of `Reproduction`: 36 programs, 630 pairs, 630000 wars, 144.7 billion steps.

| Machine | Backend | Threads per GPU | Time | Steps per second |
|:--|:--|--:|--:|--:|
| Workstation | 16 CPU threads | | 135 s | 1.07 billion |
| Workstation | one RTX 4090 | 32768 | 42 s | 3.43 billion |
| Workstation | two RTX 4090 | 32768 | 22 s | 6.50 billion |
| polaris | one R9700 | 32768 | 123 s | 1.18 billion |
| polaris | two R9700 | 32768 | 63 s | 2.31 billion |
| polaris | two R9700 | 16384 | 54 s | 2.69 billion |

A tournament of 256 programs: the 39 historical programs repeated under different names, 32640 pairs.

| Machine | Backend | Threads per GPU | Games per pair | Wars | Steps | Time | Steps per second |
|:--|:--|--:|--:|--:|--:|--:|--:|
| Workstation | two RTX 4090 | 32768 | 1000 | 32.6 million | 7.30 trillion | 1071 s | 6.82 billion |
| polaris | two R9700 | 16384 | 20 | 652800 | 146 billion | 52 s | 2.80 billion |

The speed of a GPU is limited by memory, not by the number of threads. On the RTX 4090, 32768 threads
played as fast as 65536, and 131072 at half the speed, because the 64 KB arenas of that many threads
no longer fit the caches. On the R9700, 16384 threads were 14% faster than 32768 on the full tournament.
16384 threads per GPU is the default; the RTX 4090 figures above were measured with 32768 before that.
