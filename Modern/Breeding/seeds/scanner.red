;redcode-94nop
;name Seed scanner
;author Claude (corewar Modern/Breeding)
;intent Scanner: compares two cells half a core apart and bombs both when they differ.
;assert CORESIZE==8000
step    equ 2936

scan    add.f   inc, look
look    sne.i   100, 4100
        jmp.b   scan
        mov.i   bomb, *look
        mov.i   bomb, @look
        jmp.b   scan
inc     dat.f   #step, #step
bomb    dat.f   #0, #0
        end     scan
