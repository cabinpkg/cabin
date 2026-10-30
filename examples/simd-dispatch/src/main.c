#include <stdio.h>

#ifdef HAVE_SSE41_VARIANT
int max_sse41(const int *values, int n);
#if defined(_MSC_VER)
#include <intrin.h>
static int cpu_has_sse41(void) {
    int regs[4];
    __cpuid(regs, 1);
    return (regs[2] >> 19) & 1;
}
#else
static int cpu_has_sse41(void) { return __builtin_cpu_supports("sse4.1"); }
#endif
#endif

static int max_generic(const int *values, int n) {
    int best = values[0];
    for (int i = 1; i < n; i++) {
        if (values[i] > best) {
            best = values[i];
        }
    }
    return best;
}

int main(void) {
    const int values[8] = {3, -7, 42, 5, 19, 0, -1, 8};
    const char *variant = "generic";
    int best = max_generic(values, 8);
#ifdef HAVE_SSE41_VARIANT
    if (cpu_has_sse41()) {
        variant = "sse4.1";
        best = max_sse41(values, 8);
    }
#endif
    printf("max = %d (%s)\n", best, variant);
    return best == 42 ? 0 : 1;
}
