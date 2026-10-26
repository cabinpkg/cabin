// Checks Boost.Decimal's exact decimal arithmetic, string parsing, and comparisons
// with the IEEE 754 decimal32_t and decimal64_t types.
#include <boost/decimal.hpp>

int main() {
    using boost::decimal::decimal32_t;
    using boost::decimal::decimal64_t;

    // 0.1 + 0.2 is exact in decimal floating point, unlike binary double.
    const decimal64_t a(1, -1);
    const decimal64_t b(2, -1);
    if (a + b != decimal64_t(3, -1)) return 1;

    const decimal64_t parsed("-5.25");
    if (parsed != decimal64_t(-525, -2)) return 1;
    if (boost::decimal::abs(parsed) != decimal64_t(525, -2)) return 1;
    if (!(parsed < a)) return 1;

    const decimal32_t third = decimal32_t(1) / decimal32_t(3);
    // decimal32_t carries 7 significant digits.
    if (third != decimal32_t("0.3333333")) return 1;
    if (static_cast<int>(decimal64_t(1234, -1)) != 123) return 1;
    return 0;
}
