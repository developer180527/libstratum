/* Compiled to LLVM bitcode (-flto=thin), so the linker has no object file to point the debug
 * map at. ld records the N_OSO stab with the string table's "no name" sentinel (n_strx == 0),
 * which reads as a single space rather than an empty string. `nm` and `dsymutil` drop those
 * entries; so must we, or every query carries a phantom "object not found" diagnostic.
 * See fixtures/src/debugmap/thinlto_b.cpp for the second translation unit. */

struct Wire {
    char tag;
    double value;
    int count;
};

int consume(const Wire& w);

int main(void) {
    Wire w{2, 4.5, 6};
    return consume(w);
}
