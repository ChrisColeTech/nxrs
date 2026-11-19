// Floating point through printf: %f/%e formatting (FP multiply, convert, compare in the guest's printf), and some
// arithmetic: a series, sqrt, fma, single precision, conversions.
#include "sdk.h"

int main(void) {
    printf("%f %f %.2f %.3f\n", 3.14159265, -2.5, 2.0 / 3.0, 1234.5678);
    printf("%e %e %.1f %f\n", 6.02214076e23, 0.000123, 100.0, 1e10);
    volatile int n = 1000;
    double s = 0;
    for (int k = 1; k <= n; k++) s += 1.0 / ((double)k * k);
    printf("sum 1/k^2 (k <= 1000) = %.6f\n", s);
    volatile double two = 2.0;
    printf("sqrt(2) = %.9f, fma(1.5, 2.0, 0.25) = %f\n", __builtin_sqrt(two), __builtin_fma(1.5, two, 0.25));
    volatile float f = 1.1f;
    float g = f * 3.0f;
    volatile double neg = -7.9, big = 3e9;
    printf("1.1f * 3 = %.6f, (int)-7.9 = %d, (unsigned)3e9 = %u\n", (double)g, (int)neg, (unsigned)big);
    printf("%+.2f|%8.3f|%-8.1f|%08.2f\n", 1.5, 3.14159, 2.25, -1.5);
    return 0;
}
