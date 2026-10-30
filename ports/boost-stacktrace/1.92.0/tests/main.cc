// Exercises the compiled Boost.Stacktrace archive without configuring
// it: the port's interface definition makes the headers call into it,
// so capturing, printing and the async-signal-safe dump all come from
// the library.
#include <boost/stacktrace.hpp>
#include <string>

#ifndef BOOST_STACKTRACE_LINK
#error "the port's interface definition must select the compiled archive"
#endif

int main() {
    boost::stacktrace::stacktrace st;
    if (st.empty() || !st[0].address()) return 1;
    if (boost::stacktrace::to_string(st[0]).empty()) return 1;
    if (boost::stacktrace::to_string(st).empty()) return 1;

    boost::stacktrace::frame::native_frame_ptr_t frames[16] = {};
    std::size_t n = boost::stacktrace::safe_dump_to(frames, sizeof frames);
    if (n == 0 || !frames[0]) return 1;
    return 0;
}
