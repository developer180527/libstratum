// Template instantiations whose layout depends on the arguments.

template <typename A, typename B>
struct Pair {
    A first;
    B second;
};

template <typename T, int N>
struct SmallArray {
    unsigned char size;
    T items[N];
};

Pair<char, double> g_pair_char_double;
Pair<double, char> g_pair_double_char;
SmallArray<short, 3> g_small_array;
Pair<Pair<char, int>, char> g_nested_pair;

int main() {
    return g_pair_char_double.first + g_small_array.size + g_nested_pair.second;
}
