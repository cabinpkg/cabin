// Exercises Boost.Locale through the platform's default backend, using
// only results that do not depend on the locales the host installs:
// UTF-8 <-> ISO-8859-1 conversion (iconv on POSIX, Win32 code pages on
// Windows), the info facet parsed from the locale name, ASCII case
// conversion, formatting, an untranslated message, and the Gregorian
// calendar in a fixed time zone.
#include <boost/locale.hpp>
#include <locale>
#include <string>

namespace bl = boost::locale;

int main() {
    const std::string latin1 = "\xe9t\xe9";
    const std::string utf8 = bl::conv::to_utf<char>(latin1, "ISO-8859-1");
    if (utf8 != "\xc3\xa9t\xc3\xa9") return 1;
    if (bl::conv::from_utf(utf8, "ISO-8859-1") != latin1) return 2;

    // A host without en_US falls back to the C locale, which changes
    // none of the checks below.
    bl::generator gen;
    const std::locale loc = gen("en_US.UTF-8");
    const bl::info& info = std::use_facet<bl::info>(loc);
    if (info.language() != "en" || info.country() != "US" || !info.utf8()) return 3;

    if (bl::to_upper("hello, world", loc) != "HELLO, WORLD") return 4;
    if ((bl::format("{1} holds {2} items") % "cart" % 7).str(loc) != "cart holds 7 items") return 5;
    if (bl::translate("untranslated").str(loc) != "untranslated") return 6;

    // 1971-01-01 05:00 GMT.
    const bl::date_time t(86400.0 * 365 + 5 * 3600, bl::calendar(loc, "GMT"));
    if (t / bl::period::year() != 1971 || t / bl::period::day() != 1 || t / bl::period::hour() != 5)
        return 7;
    return 0;
}
