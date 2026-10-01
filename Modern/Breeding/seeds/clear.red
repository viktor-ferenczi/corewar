;redcode-94nop
;name Seed clear
;author Claude (corewar Modern/Breeding)
;intent Core clear: a self-splitting loop writes DAT bombs forward through the whole core.
;assert CORESIZE==8000
ptr     dat.f   #0, #12
clear   spl.b   #0, #0
loop    mov.i   bomb, >ptr
        jmp.b   loop, >ptr
bomb    dat.f   <2667, <5334
        end     clear
