#include <smmintrin.h>

/* Largest of `n` ints, `n` a positive multiple of 4, through SSE4.1's
   _mm_max_epi32. */
int max_sse41(const int *values, int n) {
    __m128i best = _mm_loadu_si128((const __m128i *)values);
    for (int i = 4; i < n; i += 4) {
        best = _mm_max_epi32(best, _mm_loadu_si128((const __m128i *)(values + i)));
    }
    best = _mm_max_epi32(best, _mm_shuffle_epi32(best, _MM_SHUFFLE(1, 0, 3, 2)));
    best = _mm_max_epi32(best, _mm_shuffle_epi32(best, _MM_SHUFFLE(2, 3, 0, 1)));
    return _mm_cvtsi128_si32(best);
}
