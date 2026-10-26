// Exercises boost::logic::tribool three-state logic: indeterminate propagation
// through operators and named-state stream output.
#include <boost/logic/tribool.hpp>
#include <boost/logic/tribool_io.hpp>
#include <sstream>

int main() {
    using boost::logic::tribool;
    using boost::logic::indeterminate;
    tribool t(true), f(false), u(indeterminate);

    if (!t || f) return 1;
    if (!indeterminate(u) || indeterminate(t)) return 1;
    if (!indeterminate(t && u) || (f && u) || indeterminate(f && u)) return 1;
    if (!(t || u) || !indeterminate(f || u)) return 1;
    if (!indeterminate(!u) || !indeterminate(u == t)) return 1;

    std::ostringstream os;
    os << std::boolalpha << t << ' ' << f << ' ' << u;
    if (os.str() != "true false indeterminate") return 1;
    return 0;
}
