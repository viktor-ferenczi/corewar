# MARS in Rust

A Redcode assembler and simulator for pMARS, ICWS'88, ICWS'94, and Viktor's 1993 `MARS.COM`
(CoreWar MARS V1.0, see [`Historical`](../Historical)). The default is `pmars`, without P-space.
With `--standard hu93 --quirks --norotate` and the same random seed a battle ends exactly as in `MARS.COM`,
down to the last process count in the statistics. It needs no DOSBox, and it plays the whole tournament
of [`Reproduction`](../Reproduction) in about two minutes on the CPU instead of four hours, or in 22
seconds on two GPUs.

The exact historical preset differs from `MARS.COM` on purpose in these ways:

- The command line is new; it does not take the `/X` options of `MARS.COM`.
- The output has the same format, except for the first line, `CoreWar MARS Rust V1.0 - Viktor Ferenczi
  2026`, so you can tell which one produced it.
- `MARS.COM` ends a line only at a CR and reads an LF as a space, so a file with Unix line ends is one
  long comment for it. The port takes an LF without a CR before it as a line end too, so sources work
  with CR LF, LF and CR line ends alike. Files with DOS line ends compile exactly as in `MARS.COM`.
- Where `MARS.COM` would hang or crash while compiling, the port stops with an error.

## Standards

`--standard` applies to `compile`, `run`, and `tournament`. `--quirks` is off by default in
every standard; `94` rejects it. First movers rotate war by war in every standard. Use
`--norotate` to keep them fixed. `Settings::default()` is `pmars`; `Settings::hu93()` is
the exact historical preset, including quirks and no rotation.

### pmars

ICWS'94 with pMARS's source conventions: text `EQU`, nested `FOR`/`ROF`, loop counters,
label concatenation, `CURLINE`, registers, predefined constants, and `;assert`. `SEQ`
is a separate opcode, `NOP` defaults to `.F`, and one-operand `JMP`/`SPL`/`NOP` gets
`$0` as its B operand. P-space instructions and `PIN` are rejected. Like pMARS, it leaves a
modifier written right after its opcode (`MOV.I`) out of text substitution, so a loop
counter or `EQU` named `i` does not break it. A label in a `FOR` count is relative, an
operand can follow the opcode without a space (`MOV.AB#1,2`), and a line can be longer
than 255 characters. A false `;assert` and a start offset outside the code are errors
with messages of their own. pMARS only warns about the start offset.

`--quirks` reproduces stock pMARS 0.9.2: broken expression precedence and `==`, `w`/`s`
registers preset in the first warrior, a redefined label dropping its line, numbers
joining across spaces, immediate B fields read as fetched, and step-limit rescaling
when a warrior dies in a war with three or more warriors. It does not include the
2004 `02bimmediate` patch. Lines are read 255 bytes at a time, so the rest of a longer
line is the next line. Sources that would hang pMARS or produce garbage cells are
rejected, with or without quirks. With quirks, that includes an operand glued to the
opcode at the end of a line (`MOV#1,2`), where pMARS reads stale bytes after it.

### hu93

The 1993 instruction set, B-field arithmetic, operand timing, slot scheduler, `START`
label, shared step limit, and ownership rules. Without quirks it has a clean parser,
whole-cell `CMP`, wrapped placement with no leftovers, and a DAT test over every cell.
The compiler and simulator bugs listed below are enabled by `--quirks`.

Exact `MARS.COM` is `--standard hu93 --quirks --norotate`. It requires an 8000-cell core.
Only `hu93` supports `--no-exec-other` and the binary `--log`. Its exact statistics text
retains the historical format; other settings include the standard, quirks, and rotation.

### 88

The ICWS'88 instruction set and addressing modes, FIFO processes, and '94 register-copy
operand timing. Fields can be separated by commas or whitespace. Without a comma,
each field expression must have no internal spaces. `EQU` computes a value from earlier
labels, and `SLT A, #B` is rejected. `--quirks` matches pMARS 0.9.2 `-8`, including text
`EQU`, immediate B for `SLT`, and the pMARS quirks listed above.

### 94

The ICWS'94 draft's rules: all eight modes, modifiers, arithmetic and comparison
instructions, `ORG`, text `EQU`, and C-precedence expressions with comparisons and
logical operators. `SEQ` is synonymous with `CMP`, `NOP` defaults to `.B`, and an
absent B operand is `#0`. pMARS extensions such as registers, `FOR`, and `CURLINE`
are not accepted. Execution is checked against pMARS 0.9.5 with explicit load-file cells,
not its assembler defaults. There is no P-space and no quirks mode.

### Settings

| Setting | hu93 | pmars, 88, 94 |
|:--|--:|--:|
| `--core` | 8000 | 8000 |
| `--length` | 100, at most 100 | 100, at most 1000 |
| `--steps` | 600000 shared steps | 80000 cycles per warrior |
| `--queue` | 64, at most 256 | 8000, at most 8000 |
| `--distance` | 0 | 100 |

Options override these defaults. A zero war limit means 2^32 steps or cycles. Core size
is 100 to 65535; program length must not exceed it. Placement is seeded and obeys the
minimum start distance and nonoverlap. `--syntax hu93` compiles historical sources
with the original compiler and converts them for execution under another standard.

Quirks execution is selected once per war on the CPU using specialized code. GPU
pipelines specialize the same choices with shader constants. There is no runtime
quirks flag check in the instruction executor.

## Build and run

Tested with Rust 1.90. The GPU support (cargo feature `gpu`, on by default) uses
[wgpu](https://wgpu.rs), which runs on Vulkan on Linux, with NVIDIA and AMD GPUs alike.

Download Linux x86-64 builds from [Releases](https://github.com/viktor-ferenczi/corewar/releases).
Each release has CPU-only and GPU-enabled archives, `SHA256SUMS`, and `BUILD.json` inside each
archive with its version, tested commit, CI run, and features. The binaries are built on Ubuntu
24.04 and need glibc 2.39 or newer. The GPU build also needs a Vulkan driver.

```bash
cargo build --release
```

Without GPU support the binary is a tenth of the size and needs no Vulkan:

```bash
cargo build --release --no-default-features
```

### CI and releases

CI builds both release binaries, runs the CPU-only test suite and historical goldens, then checks
a fixed 3584-war tournament against its recorded results. Tests have one-minute limits; cold
compilation can take longer. Cargo downloads and compiled dependencies, including wgpu, are cached.
Upstream pMARS differential tests still require the reference binaries described below.

After successful CI, ready-for-review PRs get draft releases; draft PRs get none. Successful
`main` builds get published releases. Versions start at `0.1.0` and increment the patch number
for each released build, across PR and main runs. A retry reuses its run's version. The Cargo
package starts at `0.1.0`; the historical `V1.0` statistics banner is unchanged.

The publisher runs trusted code from the default branch, with no PR code or binaries executed
under release credentials. Numbering and publishing are serialized. The release workflow becomes
active once it is merged into the default branch. It can also retry a successful CI run through
the Release workflow's manual `run_id` input.

For PRs that change workflows, configure the Actions secret `RELEASE_TOKEN` with a fine-grained
token limited to this repository: Contents and Workflows write, Actions and Pull requests read.
Otherwise the publisher uses `GITHUB_TOKEN`. GitHub requires the additional Workflows permission
when a release targets a commit that changes workflows relative to the default branch.
Draft releases are visible only to collaborators; build artifacts are also available from CI
runs for 30 days. Failed, stale, closed, or newly converted-to-draft PR builds are not released.

Compile a program and list the result:

```bash
./target/release/mars compile --standard hu93 --quirks --norotate ../Historical/MICE.CWR
```

Play 500 wars and print the statistics the way `MARS /P=500` does:

```bash
./target/release/mars run --standard hu93 --quirks --norotate --wars 500 ../Historical/MICE.CWR ../Historical/KILLER.CWR
```

Options for this historical run: `--steps` (war length, 600000), `--queue` (processes per program, 64),
`--no-exec-other` (like `/E`), `--seed` (BIOS tick count, the time of day by default), `--log FILE`
(the binary statistics file of `/F`) and `--no-dat-test` (see below). `mars --help` lists them all.

## Tournaments

A tournament plays every pair of 2 to 256 programs, 1000 games per pair by default, on all CPU
cores. With rotation each pair has one run. With `--norotate` it has two runs in opposite orders:

```bash
./target/release/mars tournament --standard hu93 --quirks --norotate --out results.jsonl --seed 1 ../Historical/*.CWR
```

This historical preset plays two MARS runs per pair, with random seeds derived from the master seed
`--seed`, so a tournament can be repeated exactly. Both runs of a pair share a DOS session like in
`Reproduction/mars.py`. The file gets one JSON object per run and line, in pair order, like this one
from [`Reproduction/results/runs.jsonl`](Reproduction/results/runs.jsonl):

```json
{"first":"ANTIIMP","second":"ARTUR-1","seed":1276916,"games":500,"first_wins":0,"second_wins":73,"draws":427,"first_pcs":570,"second_pcs":30236,"steps":272061937,"max_steps":600000,"queue":64,"exec_other":true,"standard":"hu93","quirks":true,"rotate":false}
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
- `tests/gpu.rs`: the GPU against the CPU engine, war by war, for every standard, on every
  nonintegrated adapter, llvmpipe included. `MARS_TEST_GPUS=1` pins it to adapter 1;
  comma-separated indexes select several adapters. Without an adapter there is nothing to compare.
- `tests/icws88.rs`, `tests/icws94.rs`, `tests/standards.rs`: dialect and execution rules.
- `tests/pmars_diff.rs`: generated cells and expressions, assembler probes, timing probes, and
  upstream warriors against pMARS. Defaults are small; `MARS_DIFF_CASES` opts into more cases.

`cargo test --release` runs all of them in about 15 seconds. `tools/golden.py` records the reference
data again. It needs DOSBox 0.74 and nasm, and never modifies `MARS.COM`; the patched copies live in
temporary folders. It gives DOSBox the sources with CR LF line ends, whatever the checkout has.

The differential tests are offline by default. Build pMARS 0.9.2 and 0.9.5 from upstream
source, without vendoring them into this repository. In each version's `src` directory:

```bash
gcc -O2 -w -DEXT94 -DSERVER -DPERMUTATE pmars.c asm.c eval.c disasm.c cdb.c sim.c pos.c clparse.c global.c token.c str_eng.c -o ../pmars
```

Then set `PMARS_092_BIN` and `PMARS_095_BIN` to the binaries, and `PMARS_092_SRC` to the
0.9.2 source root (containing `warriors`). Run `cargo test --release --test pmars_diff`.
Reference invocations have a two-second timeout. Generated tests use explicit modifiers
and modes for `94`, avoiding pMARS's differing assembler defaults.

## Quirks it reproduces

These come from `MARS.ASM` and apply to `hu93 --quirks`. All can change results.

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
- `src/clean.rs`: clean historical parsing and draft expression arithmetic
- `src/assembler.rs`, `src/pmars_eval.rs`: standard dialects and pMARS expression evaluation
- `src/engine.rs`: the simulator (`WAR`, `WAR1`, `LOADA`, `LOADB`, the instructions)
- `src/rng.rs`: the random generator used for placement (`RANDOMIZE`, `RANDOM`)
- `src/report.rs`: a whole `MARS.COM` run in statistics mode, its text output and binary log
- `src/tournament.rs`: tournaments, placement planning, fighting on CPU threads, JSONL and `.sta` output
- `src/gpu.rs`, `src/gpu.wgsl`: fighting on GPUs
- `src/main.rs`: the command line

## Performance

### Standards benchmark

Measured on the workstation on 2026-09-30: eight historical programs, 128 games per pair,
3584 wars, seed 1, four CPU threads or RTX 4090 adapter 1. Times are wall-clock medians
of three runs, including startup. The baseline is commit `896c5b3`.

| Rules | Steps | CPU | RTX 4090 |
|:--|--:|--:|--:|
| Baseline | 1,095,410,027 | 2.82 s | 1.86 s |
| `hu93 --quirks --norotate` | 1,095,410,027 | 2.54 s | 1.66 s |
| `hu93` | 849,375,325 | 1.91 s | 1.78 s |
| `88` | 476,134,493 | 1.18 s | 0.74 s |
| `94` | 476,134,493 | 1.18 s | 0.74 s |
| `pmars` | 476,134,493 | 1.18 s | 0.74 s |

The historical preset gave identical results to the baseline on both backends, with no
slowdown. Other standards used `--syntax hu93` and their own defaults, so their results
and instruction counts differ. These short runs do not measure sustained GPU throughput.
At the default 8000-process limit, the FIFO queues add 32 KB per GPU thread next to the
64 KB arena: two 16-bit process addresses share each word. State size follows the actual
core and process limits.

### Earlier full tournaments

These measurements predate the standards change; the full tournaments were not rerun.
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
