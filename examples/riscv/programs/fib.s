# Fibonacci numbers, computed by the processor on the sheet.
#
# Writes the first 12 Fibonacci numbers to data memory, from 0x1000_0000, and leaves
# the 12th (89) in a0. ECALL ends the program, and the simulation with it.

    .text
start:
    la   t0, fib          # where the numbers go
    li   t1, 0            # F(n-2)
    li   t2, 1            # F(n-1)
    li   t3, 12           # how many
loop:
    sw   t1, 0(t0)
    add  t4, t1, t2       # F(n) = F(n-2) + F(n-1)
    mv   t1, t2
    mv   t2, t4
    addi t0, t0, 4
    addi t3, t3, -1
    bnez t3, loop
    lw   a0, -4(t0)       # the last one written
    ecall

    .data
fib:
    .space 48
