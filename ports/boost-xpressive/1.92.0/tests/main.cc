// Exercises Boost.Xpressive: a dynamic regex compiled from a string, an
// equivalent static regex built from expression templates, named and
// numbered captures, iteration over matches, replacement, and the error
// raised for an invalid pattern.
#include <boost/xpressive/xpressive.hpp>

#include <string>

namespace xp = boost::xpressive;

int main() {
    const std::string text = "boost-1.92.0 cabin-0.4.1 zlib-1.3.1";

    const xp::sregex dynamic = xp::sregex::compile("(?P<name>[a-z]+)-(\\d+)\\.(\\d+)\\.(\\d+)");
    xp::smatch what;
    if (!xp::regex_search(text, what, dynamic)) return 1;
    if (what["name"] != "boost" || what[2] != "1" || what[3] != "92" || what.position(0) != 0) return 1;

    using xp::_d;
    using xp::_w;
    const xp::sregex version = +_d >> '.' >> +_d >> '.' >> +_d;
    const xp::sregex package = (xp::s1 = +_w) >> '-' >> (xp::s2 = version);
    int count = 0;
    std::string names;
    for (xp::sregex_iterator it(text.begin(), text.end(), package), end; it != end; ++it, ++count)
        names += (*it)[1].str() + ";";
    if (count != 3 || names != "boost;cabin;zlib;") return 1;

    if (!xp::regex_match(std::string("cabin-0.4.1"), package)) return 1;
    if (xp::regex_match(std::string("cabin-0.4"), package)) return 1;
    if (xp::regex_replace(text, package, std::string("$2")) != "1.92.0 0.4.1 1.3.1") return 1;

    try {
        xp::sregex::compile("(unclosed");
        return 1;
    } catch (const xp::regex_error&) {
    }
    return 0;
}
