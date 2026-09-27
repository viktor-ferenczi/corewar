# `MARS.COM` quirks and standard conformance

This report compares `MARS.COM` ([`Historical/MARS.ASM`](../Historical/MARS.ASM)) with the Redcode
standards: ICWS'86, ICWS'88 and the 1994 draft. It lists the quirks of the implementation and says
which ones break compatibility with which standard.

In short, `MARS.COM` conforms to none of them. It implements the rules of the 1993 Hungarian
championship ([`Historical/SZABALY.TXT`](../Historical/SZABALY.TXT), in Hungarian). Those rules
copy the '88 table of legal instructions, without `SLT`, and use the '88 source syntax, but take the
'86 behavior of `ADD` and `SUB` and the '86 limit of 64 processes. Process scheduling, placement and
`CMP` work in ways of their own that match no standard. `CMP` doesn't match `SZABALY.TXT` either.

Line numbers refer to `MARS.ASM`. The exact behavior of every quirk is pinned down by the tests of
the Rust port, see [Modern](../Modern/README.md).

## Sources

Copies of these are in [Standards](Standards/README.md), which also says where they come from.

- ICWS'88: "Core War '88 - A Proposed Standard", Thomas Gettys, 1988.
  [Text](Standards/icws88.txt), [PDF](Standards/redcode88.pdf).
- ICWS'94: "Annotated Draft of the Proposed 1994 Core War Standard", v3.3, 1995.
  [Text](Standards/icws94.txt). Line numbers below are those of the draft proper.
- ICWS'86: the original text ("Core Wars", May 1986, mainly by Graeme McRae) doesn't seem to be
  online. The '86 statements here come from two secondary sources: appendix A.2.1.1 and section 4.3
  of the '94 draft, which give the '94 equivalents of '86 instructions and the '86 run-time
  settings, and Marco Pontello's
  [Guida a Core War & Redcode - ICWS-86](Standards/pontello-icws86-guide-it.txt) (in Italian).
- The 1984 [Core War Guidelines](Standards/cwg.txt) by Jones and Dewdney, for comparison.

## Differences that change battle results

### `CMP` compares B-fields only

`C_MOV` ([2466](../Historical/MARS.ASM#L2466)) copies the whole cell, but `C_CMP`
([2611](../Historical/MARS.ASM#L2611)) compares only the two values that `LOADA` and `LOADB` return,
which are the B-fields of the two target cells.

Every standard compares whole instructions when A is not immediate. '88 says so directly, and the
'94 draft translates both '86 and '88 `CMP` with non-immediate operands to `CMP.I`. `SZABALY.TXT`
also says that a non-immediate `CMP` compares all five fields.

As a result, a scanner that compares two cells takes any instruction with a B-field of 0 (`JMP -1`,
`SPL 0`, `DAT #5 #0`) for empty core, which is `DAT #0 #0`.

### `ADD` and `SUB` change the B-field only

`C_ADD` ([2498](../Historical/MARS.ASM#L2498)) and `C_SUB` ([2518](../Historical/MARS.ASM#L2518))
add the A value to the B-field of the target, whatever the A mode. This is the '86 rule: the '94
draft translates an '86 `ADD` to `ADD.B`. '88 changed it: a non-immediate `ADD` adds the A-field to
the A-field and the B-field to the B-field (`ADD.F` in '94). An '88 warrior that steps two pointers
with a single `ADD step ptr` only gets the B pointer moved here.

### When operand values are read

'88 ("Effective Address Calculations") first turns both operands into addresses, doing the `<`
decrements, and only then runs the opcode, which reads memory at that point. The '94 draft (lines
0567-0581) copies the A instruction into a register while evaluating A, before the B operand does its
decrement.

`MARS.COM` does it both ways. `LOADA` ([2675](../Historical/MARS.ASM#L2675)) returns the A address and
the B-field of the A target, then `LOADB` ([2736](../Historical/MARS.ASM#L2736)) does its decrement.
`ADD`, `SUB` and `CMP` use the saved B-field, as in '94. `MOV` reads the source cell from memory
afterwards, as in '88. For example:

```
        MOV  1  <1
        DAT  #0 #5
```

'88 and `MARS.COM` copy the already decremented `DAT #0 #4` to +5, '94 copies `DAT #0 #5`. With
`ADD 1 <1` it's the other way round: `MARS.COM` adds the old value 5 as '94 does. Real warriors rarely
hit these cases.

### Processes live in slots, not in a queue

Each program has a fixed table of PC slots (`QUEUEL` of them, [84](../Historical/MARS.ASM#L84)).
`C_SPL` ([2632](../Historical/MARS.ASM#L2632)) puts the new process into the lowest free slot, and
`WAR1` ([2313](../Historical/MARS.ASM#L2313)) goes round the slots in order, starting after the one it
ran last. All the standards keep the processes in order and put new ones at a defined place:

- '86 (per Pontello): right after the parent, so the new process runs next.
- '88: the parent's next instruction goes to the back of the queue, then the new process.
- '94: the same as '88.

When p0 of the processes p0, p1, p2 executes `SPL` and nothing dies, the following rounds run in
this order:

| Standard | Order |
| --- | --- |
| '86 | new p1 p2 p0 |
| '88, '94 | p1 p2 p0 new |
| `MARS.COM` | p1 p2 new p0 |

Once processes die, new ones fill the gaps and the order drifts further. Anything that depends on
process timing, such as imp spirals, imp gates or bombers with several processes, behaves
differently from every standard. [`Historical/MODOSIT.TXT`](../Historical/MODOSIT.TXT) shows that
keeping the PCs sorted was considered and rejected.

### Process limit

The default is 64 processes per program (`/Q`, at most 256). That is the '86 value ("ICWS86" set in
section 4.3 of the '94 draft, and Pontello). '88 sets no upper limit, and the KOTH set of the '94
draft allows 8000. `SPL` at the limit is ignored, as in '86.

### Ownership and `/E`

Each cell records the number of the program that last wrote it. `MOV`, `ADD`, `SUB`, `DJN` and even
a `<` decrement of a pointer cell claim the cell. With `/E` (off by default,
`EXECOTHER`, [85](../Historical/MARS.ASM#L85)) a process that executes a cell owned by another
program dies as if it hit a `DAT`, without evaluating the operands. So an enemy's `<` on one of your
cells makes that cell deadly for you. No standard has anything like this.

### Placement

Programs go to random positions that don't overlap ([1862](../Historical/MARS.ASM#L1862)), with no
minimum distance between them. The '94 draft makes that distance a setting: 300 for '86, 100 for
KOTH. Programs don't wrap at the end of the arena, so the part of a program placed past cell 7999
never runs and can't be hit, and a program whose start lies there never runs and can't lose.
Memory after the arena is never cleared, not even between runs of `MARS.COM` in the same DOS
session, and cells left there block later placements.

### End of the war

- The step limit (600000 by default) counts the steps of all programs together, so each of two
  programs gets 300000 cycles. A program without processes is skipped and uses no step. The
  standards count cycles per warrior and leave the number to the tournament (100000 for '86 and
  80000 for KOTH in the '94 draft), so this is a setting rather than a conformance problem.
- In statistics mode `WAR_DATTEST` ([2149](../Historical/MARS.ASM#L2149)) checks every 16000 steps
  whether any `DAT` is left and ends the war as a draw if none is. Without `/E` this is safe, since
  no instruction can create a `DAT` that doesn't exist yet, except that the check misses cell 7999.
  With `/E` it can end a war that wasn't decided yet, because processes can still die by executing
  foreign cells. Outside statistics mode the check doesn't run.

## What conforms

- The instruction set is the '86 one: the eleven '88 instructions without `SLT`. The four modes
  `#`, `$`, `@` and `<` work as in '86 and '88, with indirection through the B-field and `<`
  decrementing the B-field of the pointer cell.
- The compiler checks the '88 table of legal instructions (appendix A of the '88 text). It also
  accepts `CMP` with an immediate B and `DAT` with any mode, where '88 allows only `#` and `<` for
  `DAT`. Neither does harm. Illegal instructions can't appear during a war, because `MOV` copies
  whole cells and arithmetic only changes fields.
- Both operands are evaluated before the opcode runs, for every instruction including `DAT`, `JMP`
  and `SPL`, as '88 and '94 describe. So `JMP x <y` and `DAT <x <y` do their decrements.
- `JMZ`, `JMN` and `DJN` test the B-field. `DJN` decrements the B-field of its target, or its own
  B-field when B is immediate, as '88 says.
- Programs take turns in a fixed order. '88 requires this: "one instruction from each program is
  executed, always in the same order". The first program in the list always moves first.
- The arena starts as `DAT 0 0`, fields hold values from 0 to 7999, and a `DAT` with one operand puts
  it into the B-field.
- The arena of 8000 cells and the limit of 100 instructions are the KOTH values of the '94 draft. '88
  fixes neither, and the '86 set uses 8192 and 300.
- More than two programs in one war is an extension. '88 speaks of two programs, the '94 draft allows
  more.

## Source syntax

The syntax is close to '88 but far from '94, and most warriors published since the early 1990s are
written for '94 assemblers such as pMARS.

- Fields are separated by whitespace, as in '88. The '94 draft requires a comma between the operands
  (appendix A.2.1), and a comma is a syntax error in `MARS.COM`.
- There are no pseudo-instructions. '88 has `EQU` and `END` (with an optional entry point), '94 adds
  `ORG`. In `MARS.COM` the entry point is the label `START`, or the first instruction without it.
- A word is a mnemonic only if it is exactly three letters and a known instruction
  (`DECODEUK`, [1424](../Historical/MARS.ASM#L1424)). Any other word before the mnemonic is a label,
  and scanning for labels runs over line ends. So `END` defines a label named `END`, and `N EQU 5`
  makes `N`, `EQU` and `5` labels of the next instruction: `N` gets an address, not 5. After a label,
  even the words of a comment become labels (`LOOP ; jmp back` compiles a `JMP`).
- '88 labels start in the first column and only their first 8 characters count. `MARS.COM` accepts
  labels anywhere and compares their first 16 characters, because it reads the source through a
  16 character window.
- '88 allows `+ - * /` and doesn't say anything about precedence. `MARS.COM` also has `%`, as '94
  does later, and evaluates strictly from left to right in 16 bits (`GETPARAM`,
  [1180](../Historical/MARS.ASM#L1180)). Division treats the dividend as unsigned, so `-7/2` is 32764
  (764 in the arena) and `-1/1` crashes `MARS.COM`. '94 adds parentheses and the usual precedence;
  `MARS.COM` has neither.
- Numbers stop at 32769, a longer one is a syntax error.
- A label on instruction 59 leaves a `;` (character 59) behind, so in the first pass the rest of its
  line is taken for a comment.
- At the end of the file, whitespace after a label makes `MARS.COM` hang. A NUL byte ends the first
  pass, and the second pass then reads the rest of the read buffer first, then the file again from
  its start.

To assemble a `MARS.COM` source with an '88 assembler, add `END START` (or make `START` the first
instruction). A '94 assembler also needs commas between the operands. The result can still behave
differently because of the differences above.

## The 1984 Guidelines

`MARS.COM` has little in common with the Core War Guidelines. They have 8 instructions, no `SPL` and
no `<` mode. `JMP` takes its target from B, `DJZ` stands where `DJN` is now, and `CMP` skips when the
operands are *not* equal. Only `DAT` as the instruction that kills and relative addressing carry
over.

## Summary

| Behavior | '86 | '88 | '94 | Effect on results |
| --- | --- | --- | --- | --- |
| `CMP` on B-fields only | differs | differs | differs | yes |
| `ADD`/`SUB` on the B-field only | same | differs | differs | yes |
| `MOV` reads its source after the B decrement | unknown | same | differs | rare cases |
| `ADD`/`SUB`/`CMP` use the A value from before the B decrement | unknown | differs | same | rare cases |
| Slot scheduling | differs | differs | differs | yes, with several processes |
| 64 processes | same | differs, no limit | setting (KOTH: 8000) | yes |
| No `SLT` | same | differs | differs | only for programs that use it |
| `/E` ownership rule | none | none | none | only with `/E` |
| Placement without minimum distance or wrap | differs | not specified | setting | yes, statistically |
| Shared step limit | setting | setting | setting | little |
| DAT test in statistics mode | none | none | none | rarely |
| Fixed turn order | not known | same | same | none |
| Fields separated by whitespace | not known | same | differs | sources fail to compile |
| No `EQU`/`END`, `START` label | not known | differs | differs | sources fail or compile wrong |

"Same" and "differs" in the '86 column rest on the secondary sources listed above.

A port that has to give the same results as `MARS.COM` must keep all of this, as
[Modern](../Modern/README.md) does. Running standard warriors would need a separate '88 or
'94 mode: the scheduling, `CMP`, `ADD` and `SUB`, and the parser all differ.
