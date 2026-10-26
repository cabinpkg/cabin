// Exercises Boost.Date_Time: Gregorian calendar rules and arithmetic,
// ISO parsing and formatting, date periods and generators, and POSIX
// time points and durations, including stream output.
#include <boost/date_time/gregorian/gregorian.hpp>
#include <boost/date_time/posix_time/posix_time.hpp>

#include <sstream>

namespace g = boost::gregorian;
namespace pt = boost::posix_time;

int main() {
    const g::date leap = g::from_simple_string("2024-02-28");
    if ((leap + g::days(1)).day() != 29 || (leap + g::days(2)).month() != 3) return 1;
    if (!g::gregorian_calendar::is_leap_year(2000) || g::gregorian_calendar::is_leap_year(1900)) return 1;
    if (g::date(2026, g::Oct, 26).day_of_week() != g::Monday) return 1;
    if (g::date(2024, 1, 31) + g::months(1) != g::date(2024, 2, 29)) return 1;  // snaps to month end
    if (g::to_iso_extended_string(g::date(2026, 10, 26) + g::months(4)) != "2027-02-26") return 1;
    if ((g::date(2026, 12, 25) - g::date(2026, 1, 1)).days() != 358) return 1;

    const g::date_period q3(g::date(2026, 7, 1), g::date(2026, 10, 1));
    if (q3.length().days() != 92 || !q3.contains(g::date(2026, 9, 30)) || q3.contains(g::date(2026, 10, 1)))
        return 1;
    const g::nth_day_of_the_week_in_month fourth_thursday(
        g::nth_day_of_the_week_in_month::fourth, g::Thursday, g::Nov);
    if (fourth_thursday.get_date(2026) != g::date(2026, 11, 26)) return 1;

    const pt::ptime t = pt::time_from_string("2026-10-26 13:45:30.250");
    const pt::ptime later = t + pt::hours(12) + pt::minutes(20);
    if (later.date() != g::date(2026, 10, 27)) return 1;
    if (later.time_of_day() != pt::time_duration(2, 5, 30) + pt::milliseconds(250)) return 1;
    if (pt::to_iso_extended_string(later) != "2026-10-27T02:05:30.250000") return 1;
    if ((later - t).total_seconds() != 12 * 3600 + 20 * 60) return 1;

    std::ostringstream os;
    os << g::date(2026, 10, 26) << ' ' << pt::time_duration(1, 2, 3);
    return os.str() == "2026-Oct-26 01:02:03" ? 0 : 1;
}
