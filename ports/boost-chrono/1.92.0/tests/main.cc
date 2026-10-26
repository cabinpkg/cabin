// Exercises Boost.Chrono's compiled clocks (system, steady, process and
// thread CPU clocks) alongside duration arithmetic and conversion.
#include <boost/chrono.hpp>
#include <boost/chrono/process_cpu_clocks.hpp>
#include <boost/chrono/thread_clock.hpp>
#include <ctime>

namespace ch = boost::chrono;

int main() {
    if (ch::duration_cast<ch::milliseconds>(ch::seconds(2)).count() != 2000) return 1;
    if (ch::minutes(1) + ch::seconds(30) != ch::seconds(90)) return 1;

    ch::steady_clock::time_point s0 = ch::steady_clock::now();
#if defined(BOOST_CHRONO_HAS_PROCESS_CLOCKS)
    ch::process_cpu_clock::time_point p0 = ch::process_cpu_clock::now();
#endif
#if defined(BOOST_CHRONO_HAS_THREAD_CLOCK)
    ch::thread_clock::time_point t0 = ch::thread_clock::now();
#endif
    volatile unsigned long spin = 0;
    for (unsigned long i = 0; i < 1000000; ++i) spin += i;
    if (ch::steady_clock::now() < s0 || !ch::steady_clock::is_steady) return 1;
#if defined(BOOST_CHRONO_HAS_PROCESS_CLOCKS)
    ch::process_cpu_clock::times d = (ch::process_cpu_clock::now() - p0).count();
    if (d.real < 0 || d.user < 0 || d.system < 0) return 1;
#endif
#if defined(BOOST_CHRONO_HAS_THREAD_CLOCK)
    if (ch::thread_clock::now() < t0) return 1;
#endif

    std::time_t now = std::time(0);
    std::time_t sys = ch::system_clock::to_time_t(ch::system_clock::now());
    if (sys < now - 60 || sys > now + 60) return 1;
    return 0;
}
