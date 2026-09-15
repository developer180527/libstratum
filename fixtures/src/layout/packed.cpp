// Packed wire-format structs (#pragma pack is understood by MSVC, GCC and Clang).

#pragma pack(push, 1)
struct WireHeader {
    unsigned char version;
    unsigned int length;
    unsigned short checksum;
};
#pragma pack(pop)

#pragma pack(push, 2)
struct Pack2 {
    char a;
    int b;
    char c;
};
#pragma pack(pop)

WireHeader g_wire_header;
Pack2 g_pack2;

int main() {
    return static_cast<int>(g_wire_header.length) + g_pack2.b;
}
