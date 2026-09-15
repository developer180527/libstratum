// Padding, tail padding, and a struct that straddles a 64-byte cache line.
// Freestanding: no headers, so it builds for hosted and bare-metal targets alike.

struct Packet {
    char tag;
    double value;
    int count;
    short flags;
};

// Same members, largest alignment first: the reorder suggestion's target.
struct PacketReordered {
    double value;
    int count;
    short flags;
    char tag;
};

struct CacheStraddle {
    char header[60];
    int hot_a;     // crosses the 64-byte boundary on every target
    int hot_b;
    double cold;
};

struct Empty {};

Packet g_packet;
PacketReordered g_packet_reordered;
CacheStraddle g_cache_straddle;
Empty g_empty;

int main() {
    return g_packet.count + g_packet_reordered.count + g_cache_straddle.hot_a + static_cast<int>(sizeof(g_empty));
}
