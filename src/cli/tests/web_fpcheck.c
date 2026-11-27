// The web build's FP helpers (src/translator/runtime/simd_fp.h, WEB-W8b): under FPCR.FZ the fast paths of op / vop
// against the slow paths, bit for bit, and FFMA (the relaxed madd) against libm's correctly rounded fma, over special
// values and random bit patterns (denormals, NaNs, infinities, results around the smallest normal). Built with the
// web flags and run under emsdk's Node by tests/web_fp.rs; prints "... 0 differ ... 0 differ" and exits 0 when equal.
#include <stdio.h>
#include "simd.h"

uint32_t *rt_excl_mon;
uint32_t rt_tick_hz;
volatile int g_fnprof;
uint8_t rt_patch_on[1];
void fnprof_hit(uint64_t a, uint64_t b, uint64_t c) { (void)a, (void)b, (void)c; }
uint64_t rt_ticks(void) { return 0; }

static uint64_t st = 0x9e3779b97f4a7c15ull;
static uint64_t rnd(void) { st ^= st << 13; st ^= st >> 7; st ^= st << 17; return st; }
static const uint32_t sp32[] = {0, 0x80000000u, 1, 0x80000001u, 0x007fffffu, 0x00800000u, 0x00800001u, 0x80800000u,
    0x3f800000u, 0xbf800000u, 0x7f800000u, 0xff800000u, 0x7fc00000u, 0x7f800001u, 0x7f7fffffu, 0x40000000u,
    0x1f800000u, 0x20000000u, 0x00400000u, 0x34000000u};
static uint32_t pick32(void) {
    uint64_t r = rnd();
    switch (r & 7) {
    case 0: return sp32[(r >> 8) % (sizeof sp32 / 4)];
    case 1: return (uint32_t)(r >> 32) & 0x80ffffffu;                  // tiny / denormal
    case 2: return ((uint32_t)(r >> 32) & 0x81ffffffu) | 0x00800000u;  // around the smallest normal
    case 3: return ((uint32_t)(r >> 32) & 0x9fffffffu);                 // small exponents (tiny products)
    default: return (uint32_t)(r >> 32);
    }
}
static uint64_t pick64(void) {
    uint64_t r = rnd();
    switch (r & 7) {
    case 0: return (uint64_t)sp32[(r >> 8) % (sizeof sp32 / 4)] << 32;
    case 1: return rnd() & 0x800fffffffffffffull;
    case 2: return (rnd() & 0x801fffffffffffffull) | 0x0010000000000000ull;
    case 3: return rnd() & 0x87ffffffffffffffull;
    default: return rnd();
    }
}

int main(void) {
    static Cpu cs;
    Cpu *c = &cs;
    long bad = 0, n = 0, fmabad = 0;
    const int ops[] = {FOP_ADD, FOP_SUB, FOP_MUL, FOP_MULX, FOP_NMUL, FOP_DIV, FOP_ABD, FOP_SQRT, FOP_MADD, FOP_RECPS};
    for (int dn = 0; dn < 2; dn++) {
        c->fpcr = FPCR_FZ | (dn ? FPCR_DN : 0);
        for (int it = 0; it < 400000; it++) {
            int op = ops[it % (sizeof ops / sizeof *ops)];
            vf32 a, b, d;
            for (int i = 0; i < 4; i++) a[i] = fv_s(pick32()), b[i] = fv_s(pick32()), d[i] = fv_s(pick32());
            vf32 rv = vop_s(c, op, a, b, d), sv = vslow_s(c, op, a, b, d);
            for (int i = 0; i < 4; i++) {
                float r = op_s(c, op, a[i], b[i], d[i]), s = slow_s(c, op, a[i], b[i], d[i]);
                n += 2;
                int miss = (fb_s(r) != fb_s(s)) + (fb_s(rv[i]) != fb_s(sv[i]));
                if (miss && bad < 10)
                    printf("f32 op %d %08x %08x %08x: fast %08x vector %08x slow %08x\n", op, fb_s(a[i]), fb_s(b[i]),
                           fb_s(d[i]), fb_s(r), fb_s(rv[i]), fb_s(s));
                bad += miss;
            }
            vf64 x, y, z;
            for (int i = 0; i < 2; i++) x[i] = fv_d(pick64()), y[i] = fv_d(pick64()), z[i] = fv_d(pick64());
            vf64 rx = vop_d(c, op, x, y, z), sx = vslow_d(c, op, x, y, z);
            for (int i = 0; i < 2; i++) {
                double r = op_d(c, op, x[i], y[i], z[i]), s = slow_d(c, op, x[i], y[i], z[i]);
                n += 2;
                bad += (fb_d(r) != fb_d(s)) + (fb_d(rx[i]) != fb_d(sx[i]));
            }
        }
    }
    for (int it = 0; it < 1000000; it++) {
        float p = fv_s(pick32()), q = fv_s(pick32()), r = fv_s(pick32());
        float a = rmadd_s_s(p, q, r), e = fmaf(p, q, r);
        if (fb_s(a) != fb_s(e) && !(a != a && e != e)) fmabad++;
        double u = fv_d(pick64()), v = fv_d(pick64()), w = fv_d(pick64());
        double b = rmadd_s_d(u, v, w), f = fma(u, v, w);
        if (fb_d(b) != fb_d(f) && !(b != b && f != f)) fmabad++;
    }
    printf("FZ fast paths vs slow paths: %ld results, %ld differ; FFMA vs libm fma: 2000000 results, %ld differ\n", n,
           bad, fmabad);
    return bad || fmabad;
}
