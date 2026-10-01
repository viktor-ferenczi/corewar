;redcode-94nop
;name Seed stone
;author Claude (corewar Modern/Breeding)
;intent Mod-4 bomber. The targets share the bomb's residue, so it never hits its own code.
;assert CORESIZE==8000
step    equ 3044

start   add.ab  #step, hit
hit     mov.i   bomb, bomb
        jmp.b   start
bomb    dat.f   #0, #0
        end     start
