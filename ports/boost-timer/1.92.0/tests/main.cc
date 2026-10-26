// Exercises Boost.Timer's compiled cpu_timer, its formatting, and
// auto_cpu_timer's report on destruction.
#include <boost/timer/timer.hpp>
#include <sstream>
#include <string>

int main() {
    boost::timer::cpu_timer timer;
    if (timer.is_stopped()) return 1;
    volatile unsigned long spin = 0;
    for (unsigned long i = 0; i < 1000000; ++i) spin += i;
    timer.stop();
    if (!timer.is_stopped()) return 1;

    boost::timer::cpu_times t = timer.elapsed();
    if (t.wall < 0 || t.user < 0 || t.system < 0) return 1;
    if (timer.elapsed().wall != t.wall) return 1;  // stopped timers stay frozen
    if (timer.format(3, "%w|%u|%s").find('|') == std::string::npos) return 1;

    timer.resume();
    if (timer.is_stopped()) return 1;

    std::ostringstream out;
    {
        boost::timer::auto_cpu_timer report(out, 2, "elapsed %w s\n");
    }
    return out.str().compare(0, 8, "elapsed ") == 0 ? 0 : 1;
}
