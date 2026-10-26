// Exercises boost::optional: engaged/disengaged states, value_or, reset, comparison with
// boost::none, bad_optional_access, and optional references.
#include <boost/optional.hpp>
#include <string>

static boost::optional<int> parse_digit(char c) {
    if (c < '0' || c > '9') return boost::none;
    return c - '0';
}

int main() {
    boost::optional<int> seven = parse_digit('7');
    boost::optional<int> bad = parse_digit('x');
    if (!seven || *seven != 7) return 1;
    if (bad || bad != boost::none) return 1;
    if (bad.value_or(-1) != -1) return 1;

    bool threw = false;
    try {
        (void)bad.value();
    } catch (const boost::bad_optional_access&) {
        threw = true;
    }
    if (!threw) return 1;

    boost::optional<std::string> s(std::string("hi"));
    s->append("!");
    if (*s != "hi!" || s->size() != 3) return 1;
    s.reset();
    if (s.has_value()) return 1;

    int target = 1;
    boost::optional<int&> ref(target);
    *ref = 5;
    if (target != 5) return 1;
    return 0;
}
