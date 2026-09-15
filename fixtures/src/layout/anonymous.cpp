// Anonymous unions and structs, nested types, explicit alignment, no_unique_address.

#if defined(_MSC_VER)
#define NO_UNIQUE_ADDRESS [[msvc::no_unique_address]]
#else
#define NO_UNIQUE_ADDRESS [[no_unique_address]]
#endif

struct Variant {
    unsigned char kind;
    union {
        int i;
        float f;
        struct {
            short x;
            short y;
        } point;
    };
};

namespace engine {
struct Outer {
    struct Inner {
        char c;
        int n;
    };
    Inner first;
    Inner second;
};
}  // namespace engine

struct Aligned {
    char c;
    alignas(16) int aligned_int;
};

struct Allocator {};

struct Holder {
    NO_UNIQUE_ADDRESS Allocator alloc;
    int* data;
};

Variant g_variant;
engine::Outer g_outer;
Aligned g_aligned;
Holder g_holder;

int main() {
    return g_variant.i + g_outer.second.n + g_aligned.aligned_int + (g_holder.data ? 1 : 0);
}
