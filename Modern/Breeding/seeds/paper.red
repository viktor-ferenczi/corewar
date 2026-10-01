;redcode-94nop
;name Seed paper
;author Claude (corewar Modern/Breeding)
;intent Replicator: four processes copy the body to a new place and split there.
;assert CORESIZE==8000
dist    equ 2331

start   spl.b   1
        spl.b   1
body    spl.b   @body, }dist
        mov.i   }body, >body
        mov.i   bomb, >1777
        jmp.b   body, <body-300
bomb    dat.f   <2667, <5334
        end     start
