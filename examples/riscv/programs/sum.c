/* A C program for the processor on the sheet. Set IMEM's program to programs/sum.c:
 * Circuiter compiles it with clang when you press Run. main's return value ends up
 * in register a0 (x10) when the program finishes.
 */

static int squares[10];

int main(void) {
    int total = 0;
    for (int i = 0; i < 10; i++) {
        squares[i] = i * i;
        total += squares[i];
    }
    return total; /* 285 */
}
