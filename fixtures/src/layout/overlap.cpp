// Members sharing storage at the smallest sizes, next to sharing that is legitimate.
// PDBs flatten anonymous unions into the enclosing struct; DWARF keeps one anonymous member.

#if defined(_MSC_VER)
#define NO_UNIQUE_ADDRESS [[msvc::no_unique_address]]
#else
#define NO_UNIQUE_ADDRESS [[no_unique_address]]
#endif

struct TinyUnion {
    unsigned char tag;
    union {
        char a;
        char b;
    };
};

struct CharAndBits {
    union {
        char c;
        unsigned char bits : 3;
    };
    unsigned char tail;
};

struct Nothing {};

struct EmptyAndChar {
    NO_UNIQUE_ADDRESS Nothing e;  // may share an address with `c`: not an overlap
    char c;
};

TinyUnion g_tiny;
CharAndBits g_char_and_bits;
EmptyAndChar g_empty_and_char;

int main() {
    return g_tiny.a + g_char_and_bits.c + g_empty_and_char.c;
}
