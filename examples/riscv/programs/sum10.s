# RISC-V assembly. ECALL ends the program.

    .text
start:
    li a0, 0
	li t0, 1
	li t1, 11

loop:
	add a0, a0, t0
	addi t0, t0, 1
	bne t0, t1, loop

    ecall