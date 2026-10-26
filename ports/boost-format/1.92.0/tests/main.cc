// Formats values with Boost.Format positional and printf-style directives and
// checks argument-count errors are reported.
#include <boost/format.hpp>
#include <string>

int main() {
    if (boost::str(boost::format("%1% + %1% = %2%") % 2 % 4) != "2 + 2 = 4") return 1;
    if (boost::str(boost::format("%05d|%-4s|%x") % 42 % "ab" % 255) != "00042|ab  |ff") return 1;
    if (boost::str(boost::format("%.2f") % 3.14159) != "3.14") return 1;

    boost::format reusable("<%1%>");
    reusable % "a";
    if (reusable.str() != "<a>") return 1;
    reusable % "b";
    if (reusable.str() != "<b>") return 1;

    try {
        boost::format f("%1% %2%");
        f % 1;
        (void)f.str();
        return 1;
    } catch (boost::io::too_few_args&) {
    }
    return 0;
}
