// Exercises Boost.StaticString: fixed-capacity strings with append, find, substr, insert,
// to_static_string, comparison, and length_error on capacity overflow.
#include <boost/static_string.hpp>
#include <stdexcept>
#include <string>

int main() {
    boost::static_string<16> s = "hello";
    if (s.size() != 5 || s.capacity() != 16) return 1;
    s += ", world";
    if (s != "hello, world" || s.find("world") != 7) return 1;
    if (s.substr(0, 5) != "hello") return 1;
    s.insert(5, "!!");
    if (std::string(s.c_str()) != "hello!!, world") return 1;

    boost::static_string<11> n = boost::static_strings::to_static_string(-1234);
    if (n != "-1234") return 1;

    try {
        s.append("too long for the remaining room");
        return 1;
    } catch (const std::length_error&) {
    }
    if (s.size() != 14) return 1;
    return 0;
}
