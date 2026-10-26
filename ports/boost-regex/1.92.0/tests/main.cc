// Exercises Boost.Regex: regex_search with capture groups, regex_match, regex_replace,
// and regex_error on an invalid pattern.
#include <boost/regex.hpp>
#include <string>

int main() {
    const boost::regex date("(\\d{4})-(\\d{2})-(\\d{2})");
    const std::string text = "released on 2026-10-26.";
    boost::smatch m;
    if (!boost::regex_search(text, m, date)) return 1;
    if (m.size() != 4 || m[0] != "2026-10-26" || m[1] != "2026" || m[3] != "26") return 1;

    if (!boost::regex_match(std::string("abc123"), boost::regex("[a-z]+\\d+"))) return 1;
    if (boost::regex_match(std::string("abc"), boost::regex("\\d+"))) return 1;

    const std::string swapped =
        boost::regex_replace(std::string("x=1, y=2"), boost::regex("(\\w)=(\\d)"), "$2=$1");
    if (swapped != "1=x, 2=y") return 1;

    try {
        boost::regex bad("(unclosed");
        return 1;
    } catch (const boost::regex_error&) {
    }
    return 0;
}
