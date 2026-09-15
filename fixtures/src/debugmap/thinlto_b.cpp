/* Second bitcode translation unit, so the link has more than one LTO input.
 * See fixtures/src/debugmap/thinlto.cpp. */

struct Wire {
    char tag;
    double value;
    int count;
};

int consume(const Wire& w) {
    return w.tag + static_cast<int>(w.value) + w.count;
}
