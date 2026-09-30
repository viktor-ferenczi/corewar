# CoreWar

## Historical

Historical implementations and programs from 1993.

The folder also contains CoreWar programs from that era, including mine and some from others.

## Reproduction

A 2026 tournament between the programs in `Historical`, played with `MARS.COM` under DOSBox.
It is not the original 1993 competition data. See [Reproduction](Reproduction/README.md) for details,
and the published results at https://viktor-ferenczi.github.io/corewar/

## Modern

[Modern](Modern/README.md) is a native Rust assembler and simulator for pMARS, ICWS'88,
ICWS'94, and historical `MARS.COM` rules. It defaults to `pmars`, without P-space.
Its golden tests check exact `MARS.COM` results with `--standard hu93 --quirks --norotate`. The tournament of
`Reproduction` played again on it is in [Modern/Reproduction](Modern/Reproduction/README.md).

## `MARS.COM` and `MARS.ASM`

My own implementation of the MARS assembler and emulator. Comments are in half English and
half Hungarian. (I'm not a native English speaker.)

It was compiled by MASM (Microsoft Assembler) and ran on a 80286 PC with MS-DOS 6.22.

Today you can run it by the excellent `dosbox` emulator.

This MARS compiler and emulator granted me a significant advantage in the 1993
competition in Budapest, Hungary. It was a lot faster than other implementations,
and the compiler was correct and very fast. It allowed me to "breed" programs more
easily, since I could quickly see which ones performed better in a moderate,
but already statistically significant number of competition rounds. In today's
language it allowed for agile development.

[Docs/Quirks.md](Docs/Quirks.md) lists the quirks of `MARS.COM` and compares it with the ICWS'86,
'88 and '94 Redcode standards. Copies of the standards are in [Docs/Standards](Docs/Standards/README.md).

## `COREWAR.EXE` and `COREWAR.HLP`

The CoreWar "reference" implementation I've got without sources, made by Kovacs Tamas in 1993.
