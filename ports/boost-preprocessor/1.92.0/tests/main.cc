// Exercises Boost.Preprocessor: arithmetic, token pasting, stringizing, repetition, and
// sequence iteration, checked in #if and at runtime.
#include <boost/preprocessor.hpp>
#include <cstring>

#if BOOST_PP_ADD(2, 3) != 5 || BOOST_PP_MUL(4, 5) != 20 || BOOST_PP_SEQ_SIZE((a)(b)(c)) != 3
#error "Boost.Preprocessor arithmetic is broken"
#endif

#define DECL(z, n, unused) int BOOST_PP_CAT(v, n) = n * n;
#define ADD_ELEM(r, acc, elem) + elem

int main() {
    BOOST_PP_REPEAT(4, DECL, ~)
    if (v0 + v1 + v2 + v3 != 14) return 1;

    int values[] = { BOOST_PP_ENUM_PARAMS(3, 1) }; // 10, 11, 12
    if (sizeof(values) / sizeof(values[0]) != 3 || values[0] != 10 || values[2] != 12) return 1;

    int total = 0 BOOST_PP_SEQ_FOR_EACH(ADD_ELEM, ~, (10)(20)(30));
    if (total != 60) return 1;

    if (std::strcmp(BOOST_PP_STRINGIZE(BOOST_PP_CAT(foo, bar)), "foobar") != 0) return 1;
    if (BOOST_PP_IF(1, 7, 9) != 7 || BOOST_PP_TUPLE_ELEM(3, 1, (4, 5, 6)) != 5) return 1;
    return 0;
}
