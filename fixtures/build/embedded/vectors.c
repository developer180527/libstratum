/* Vector table and reset handler for freestanding fixtures.
 * In C, casting an address to a function pointer is a valid static initializer,
 * so the table is emitted as constant data in .isr_vector. */

extern char __stack_top;
int main(void);

void reset_handler(void) {
    main();
    for (;;) {
    }
}

__attribute__((section(".isr_vector"), used))
void (*const vector_table[2])(void) = {(void (*)(void))&__stack_top, reset_handler};
