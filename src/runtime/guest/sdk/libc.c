// The libc subset the tests use: memory and string functions, a heap from svcSetHeapSize, printf to
// svcOutputDebugString (integers, strings, characters, pointers, %f and %e).
#include "sdk.h"

void *memcpy(void *d, const void *s, size_t n) {
    uint8_t *dp = d;
    const uint8_t *sp = s;
    while (n--) *dp++ = *sp++;
    return d;
}
void *memmove(void *d, const void *s, size_t n) {
    uint8_t *dp = d;
    const uint8_t *sp = s;
    if (dp < sp)
        while (n--) *dp++ = *sp++;
    else
        while (n--) dp[n] = sp[n];
    return d;
}
void *memset(void *d, int c, size_t n) {
    uint8_t *p = d;
    while (n--) *p++ = (uint8_t)c;
    return d;
}
int memcmp(const void *a, const void *b, size_t n) {
    const uint8_t *x = a, *y = b;
    for (; n; n--, x++, y++)
        if (*x != *y) return *x - *y;
    return 0;
}
size_t strlen(const char *s) {
    size_t n = 0;
    while (s[n]) n++;
    return n;
}
int strcmp(const char *a, const char *b) {
    while (*a && *a == *b) a++, b++;
    return (uint8_t)*a - (uint8_t)*b;
}

// ---- heap: a bump allocator over the process heap (free keeps nothing; the tests allocate little)
static uint8_t *heap_cur, *heap_end;

void __heap_init(void) {
    uint64_t addr = 0, size = 64 << 20;
    if (svcSetHeapSize(&addr, size)) abort();
    heap_cur = (uint8_t *)addr;
    heap_end = heap_cur + size;
}

void *malloc(size_t n) {
    n = (n + 15) & ~(size_t)15;
    uint8_t *p = __atomic_fetch_add(&heap_cur, n, __ATOMIC_SEQ_CST);
    if (p + n > heap_end) abort();
    return p;
}
void free(void *p) { (void)p; }

// ---- printf
typedef struct {
    char *buf;
    size_t size, len;
} Out;

static void put(Out *o, char c) {
    if (o->len + 1 < o->size) o->buf[o->len] = c;
    o->len++;
}

static void put_str(Out *o, const char *s, int width, int left) {
    int n = (int)strlen(s);
    if (!left)
        for (; width > n; width--) put(o, ' ');
    while (*s) put(o, *s++);
    for (; width > n; width--) put(o, ' ');
}

static void fmt_u64(char *tmp, uint64_t v, unsigned base, int upper) {
    char rev[24];
    int n = 0;
    do {
        unsigned d = (unsigned)(v % base);
        rev[n++] = (char)(d < 10 ? '0' + d : (upper ? 'A' : 'a') + d - 10);
        v /= base;
    } while (v);
    for (int i = 0; i < n; i++) tmp[i] = rev[n - 1 - i];
    tmp[n] = 0;
}

static const double pow10_table[] = {1e0, 1e1, 1e2, 1e3, 1e4, 1e5, 1e6, 1e7, 1e8, 1e9, 1e10, 1e11, 1e12, 1e13, 1e14, 1e15};

// x >= 0, finite, x < 2^63: "<int>.<prec digits>"
static void fmt_fixed(char *tmp, double x, int prec) {
    uint64_t ip = (uint64_t)x;
    double frac = x - (double)ip;
    uint64_t scale = (uint64_t)pow10_table[prec];
    uint64_t f = (uint64_t)(frac * (double)scale + 0.5);
    if (f >= scale) ip++, f -= scale;
    fmt_u64(tmp, ip, 10, 0);
    if (prec) {
        char *p = tmp + strlen(tmp);
        *p++ = '.';
        for (int i = prec - 1; i >= 0; i--) p[i] = (char)('0' + f % 10), f /= 10;
        p[prec] = 0;
    }
}

static void fmt_double(char *tmp, double x, int prec, char conv) {
    char *p = tmp;
    if (x != x) {
        memcpy(tmp, "nan", 4);
        return;
    }
    if (x < 0 || (x == 0 && 1 / x < 0)) *p++ = '-', x = -x;
    if (x == __builtin_inf()) {
        memcpy(p, "inf", 4);
        return;
    }
    if (prec > 15) prec = 15;
    if (conv == 'f' && x < 9.2e18) {
        fmt_fixed(p, x, prec);
        return;
    }
    int e = 0;
    if (x != 0) {
        while (x >= 10) x /= 10, e++;
        while (x < 1) x *= 10, e--;
    }
    fmt_fixed(p, x, prec);
    if (p[0] == '1' && p[1] == '0') {   // 9.99.. rounded up to 10.0..
        x /= 10, e++;
        fmt_fixed(p, x, prec);
    }
    p += strlen(p);
    *p++ = 'e';
    *p++ = e < 0 ? '-' : '+';
    if (e < 0) e = -e;
    if (e < 10) *p++ = '0';
    fmt_u64(p, (uint64_t)e, 10, 0);
}

int vsnprintf(char *buf, size_t size, const char *fmt, va_list ap) {
    Out o = {buf, size, 0};
    for (; *fmt; fmt++) {
        if (*fmt != '%') {
            put(&o, *fmt);
            continue;
        }
        fmt++;
        int left = 0, zero = 0, plus = 0, width = 0, prec = -1, lng = 0;
        for (;; fmt++) {
            if (*fmt == '-') left = 1;
            else if (*fmt == '0') zero = 1;
            else if (*fmt == '+') plus = 1;
            else break;
        }
        if (*fmt == '*') width = va_arg(ap, int), fmt++;
        while (*fmt >= '0' && *fmt <= '9') width = width * 10 + (*fmt++ - '0');
        if (*fmt == '.') {
            prec = 0;
            fmt++;
            while (*fmt >= '0' && *fmt <= '9') prec = prec * 10 + (*fmt++ - '0');
        }
        while (*fmt == 'l' || *fmt == 'z' || *fmt == 'h') lng += *fmt == 'l' || *fmt == 'z', fmt++;
        char tmp[64], num[64];
        const char *s = tmp;
        switch (*fmt) {
        case 'd':
        case 'i': {
            int64_t v = lng ? va_arg(ap, int64_t) : va_arg(ap, int);
            char *p = tmp;
            if (v < 0) *p++ = '-', v = -v;
            else if (plus) *p++ = '+';
            fmt_u64(p, (uint64_t)v, 10, 0);
            break;
        }
        case 'u':
        case 'x':
        case 'X': {
            uint64_t v = lng ? va_arg(ap, uint64_t) : va_arg(ap, unsigned);
            fmt_u64(tmp, v, *fmt == 'u' ? 10 : 16, *fmt == 'X');
            break;
        }
        case 'p':
            tmp[0] = '0', tmp[1] = 'x';
            fmt_u64(tmp + 2, (uint64_t)va_arg(ap, void *), 16, 0);
            break;
        case 'c':
            tmp[0] = (char)va_arg(ap, int), tmp[1] = 0;
            break;
        case 's':
            s = va_arg(ap, const char *);
            if (!s) s = "(null)";
            break;
        case 'f':
        case 'e': {
            double v = va_arg(ap, double);
            fmt_double(num, v, prec < 0 ? 6 : prec, *fmt);
            char *p = tmp;
            if (plus && num[0] != '-') *p++ = '+';
            memcpy(p, num, strlen(num) + 1);
            break;
        }
        case '%':
            tmp[0] = '%', tmp[1] = 0;
            break;
        default:
            tmp[0] = '?', tmp[1] = 0;
        }
        if (zero && !left && *fmt != 's' && *fmt != 'c') {
            int n = (int)strlen(s);
            const char *digits = s;
            if (*digits == '-' || *digits == '+') put(&o, *digits++), width--, n--;
            for (; width > n; width--) put(&o, '0');
            put_str(&o, digits, 0, 0);
        } else {
            put_str(&o, s, width, left);
        }
    }
    if (o.size) o.buf[o.len < o.size ? o.len : o.size - 1] = 0;
    return (int)o.len;
}

int snprintf(char *buf, size_t size, const char *fmt, ...) {
    va_list ap;
    va_start(ap, fmt);
    int n = vsnprintf(buf, size, fmt, ap);
    va_end(ap);
    return n;
}

int printf(const char *fmt, ...) {
    char buf[1024];
    va_list ap;
    va_start(ap, fmt);
    int n = vsnprintf(buf, sizeof buf, fmt, ap);
    va_end(ap);
    svcOutputDebugString(buf, n < (int)sizeof buf ? (uint64_t)n : sizeof buf - 1);
    return n;
}

int puts(const char *s) { return printf("%s\n", s); }

void abort(void) {
    static const char m[] = "abort()\n";
    svcOutputDebugString(m, sizeof m - 1);
    __builtin_trap();
}

void exit(int code) {
    (void)code;
    svcExitProcess();
}
