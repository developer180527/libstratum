// Base classes: empty base optimization, multiple inheritance, vtable pointers, virtual bases.

struct EmptyTag {};

struct WithEmptyBase : EmptyTag {
    int value;
};

struct Left {
    int left;
};

struct Right {
    double right;
};

struct Multiple : Left, Right {
    char own;
};

struct Polymorphic {
    virtual ~Polymorphic() {}
    virtual int id() const { return 1; }
    char small;
    long long big;
};

struct Derived : Polymorphic {
    int id() const override { return 2; }
    short extra;
};

struct VBase {
    int shared;
};

struct VLeft : virtual VBase {
    int l;
};

struct VRight : virtual VBase {
    int r;
};

struct Diamond : VLeft, VRight {
    int d;
};

WithEmptyBase g_with_empty_base;
Multiple g_multiple;
Derived g_derived;
Diamond g_diamond;

int main() {
    Polymorphic* p = &g_derived;
    return p->id() + g_multiple.left + g_diamond.shared + g_with_empty_base.value;
}
