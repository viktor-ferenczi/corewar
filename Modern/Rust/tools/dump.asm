; Compiled program dumper patched into a copy of MARS.COM by golden.py.
;
; It overwrites the usage text (M_NOPROG at 0236H, only printed when no program is given) and
; is called instead of "MOV AX,13H / INT 10H / CALL WAR" in ENTRY, after the programs compiled
; without errors. It writes the first 828 bytes of every PROGDATA structure (the header and the
; COMPILED array) to DUMP.BIN, then returns into QUIT without playing.
;
; nasm -f bin -o dump.bin dump.asm

bits 16
org 0x236

NPROG0   equ 0x2280
FIRSTSEG equ 0x2286
NEXTSEG  equ 2
DUMPLEN  equ 28 + 100 * 8

dump:
	mov ah, 0x3C
	xor cx, cx
	mov dx, fname
	int 0x21
	jc .quit
	mov bx, ax
	mov cx, [NPROG0]
	mov ax, [FIRSTSEG]
.next:
	push cx
	push ds
	mov ds, ax
	mov ah, 0x40
	mov cx, DUMPLEN
	xor dx, dx
	int 0x21
	mov ax, [NEXTSEG]
	pop ds
	pop cx
	loop .next
	mov ah, 0x3E
	int 0x21
.quit:
	ret

fname db "DUMP.BIN", 0
