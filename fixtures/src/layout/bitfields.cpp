// Bitfields: mixed widths, a zero-width separator, bool and enum bitfields, and storage-unit spill.

enum Mode : unsigned char { ModeOff, ModeOn, ModeAuto };

struct Flags {
    unsigned int a : 3;
    unsigned int b : 5;
    unsigned int : 0;          // forces the next field onto a new storage unit
    unsigned int c : 7;
    bool enabled : 1;
    Mode mode : 2;
    unsigned long long wide : 40;
    unsigned char tail;
};

struct RegisterBits {          // memory-mapped register style
    unsigned int enable : 1;
    unsigned int reserved0 : 7;
    unsigned int prescaler : 8;
    unsigned int reserved1 : 16;
};

Flags g_flags;
RegisterBits g_register_bits;

int main() {
    return static_cast<int>(g_flags.c + g_register_bits.prescaler);
}
