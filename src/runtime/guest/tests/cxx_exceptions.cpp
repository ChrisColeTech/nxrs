// C++ exceptions through DWARF unwinding: an int, a derived class caught by base reference, a C string, and
// catch (...); destructors of the frames unwound run (cleanup landing pads, then _Unwind_Resume).
extern "C" int printf(const char *, ...);

struct Base {
    int code;
};
struct Derived : Base {
    const char *what;
};
struct Guard {
    const char *name;
    ~Guard() { printf("~Guard(%s)\n", name); }
};

__attribute__((noinline)) void thrower(int k) {
    Guard g{"thrower"};
    if (k == 0) throw 42;
    if (k == 1) {
        Derived d;
        d.code = 7;
        d.what = "derived";
        throw d;
    }
    if (k == 2) throw "c-string";
}

__attribute__((noinline)) void middle(int k) {
    Guard g{"middle"};
    thrower(k);
    printf("not reached\n");
}

extern "C" int main() {
    try {
        middle(0);
    } catch (int v) {
        printf("caught int %d\n", v);
    }
    try {
        middle(1);
    } catch (Base &b) {
        printf("caught Base& code %d\n", b.code);
    }
    try {
        middle(2);
    } catch (const char *s) {
        printf("caught const char* %s\n", s);
    }
    try {
        thrower(0);
    } catch (...) {
        printf("caught ...\n");
    }
    printf("done\n");
    return 0;
}
