/* Plain C: struct, union, enum, and a flexible array member. */

enum Color { Red, Green, Blue };

struct Point {
    char label;
    int x;
    int y;
};

union Number {
    char c;
    int i;
    double d;
};

struct Message {
    unsigned short length;
    enum Color color;
    char payload[];
};

struct Point g_point;
union Number g_number;
enum Color g_color;
struct Message* g_message;

int main(void) {
    return g_point.x + g_number.i + (int)g_color + (g_message ? 1 : 0);
}
