// Exercises Boost.Charconv's compiled floating-point conversions:
// shortest round-trip to_chars, fixed and hex formats, from_chars
// parsing, and its error reporting.
#include <boost/charconv.hpp>
#include <cstring>
#include <string>
#include <system_error>

namespace cc = boost::charconv;

static std::string format(double value, cc::chars_format fmt, int precision = -1) {
    char buf[64];
    cc::to_chars_result r = precision < 0 ? cc::to_chars(buf, buf + sizeof buf, value, fmt)
                                          : cc::to_chars(buf, buf + sizeof buf, value, fmt, precision);
    return r.ec == std::errc() ? std::string(buf, r.ptr) : std::string();
}

int main() {
    if (format(0.1, cc::chars_format::general) != "0.1") return 1;
    if (format(1e300, cc::chars_format::general) != "1e+300") return 1;
    if (format(3.14159, cc::chars_format::fixed, 2) != "3.14") return 1;
    if (format(1.0, cc::chars_format::hex) != "1p+0") return 1;

    char tiny[2];
    if (cc::to_chars(tiny, tiny + 2, 1.5f).ec != std::errc::value_too_large) return 1;

    float f = 0;
    const char* text = "2.25 tail";
    cc::from_chars_result r = cc::from_chars(text, text + std::strlen(text), f);
    if (r.ec != std::errc() || f != 2.25f || r.ptr != text + 4) return 1;

    double d = 0;
    const char* hex = "-2a";
    r = cc::from_chars(hex, hex + std::strlen(hex), d, cc::chars_format::hex);
    if (r.ec != std::errc() || d != -42.0) return 1;

    const char* huge = "1e400";
    if (cc::from_chars(huge, huge + 5, d).ec != std::errc::result_out_of_range) return 1;
    const char* bad = "x1";
    if (cc::from_chars(bad, bad + 2, d).ec != std::errc::invalid_argument) return 1;
    return 0;
}
