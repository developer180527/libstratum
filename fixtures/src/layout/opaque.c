/* A struct that is declared but never defined: debug info has only a declaration
 * (DWARF DW_AT_declaration, PDB forward reference). Used through a pointer. */

struct Opaque;

struct Defined {
    int x;
};

struct Opaque* g_opaque;
struct Defined g_defined;

int main(void) {
    return g_opaque ? 1 : g_defined.x;
}
