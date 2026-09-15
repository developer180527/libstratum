// Minimal runtime support so freestanding C++ fixtures link without a C library.
// Not part of what fixtures test; excluded from goldens by name.

using size_t = __SIZE_TYPE__;

extern "C" {
void* memset(void* dst, int value, size_t size) {
    unsigned char* p = static_cast<unsigned char*>(dst);
    while (size--) *p++ = static_cast<unsigned char>(value);
    return dst;
}
void* memcpy(void* dst, const void* src, size_t size) {
    unsigned char* d = static_cast<unsigned char*>(dst);
    const unsigned char* s = static_cast<const unsigned char*>(src);
    while (size--) *d++ = *s++;
    return dst;
}
void __cxa_pure_virtual() {
    for (;;) {}
}
// Global destructor registration: GCC's Arm EABI uses __aeabi_atexit, Clang uses __cxa_atexit.
int __aeabi_atexit(void*, void (*)(void*), void*) {
    return 0;
}
int __cxa_atexit(void (*)(void*), void*, void*) {
    return 0;
}
void* __dso_handle = nullptr;
}

void operator delete(void*) noexcept {}
void operator delete(void*, size_t) noexcept {}
