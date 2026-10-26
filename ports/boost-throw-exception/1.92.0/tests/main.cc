// Exercises Boost.ThrowException: BOOST_THROW_EXCEPTION wraps the exception so it is catchable
// as itself and as boost::exception, and records the throw location.
#include <boost/throw_exception.hpp>
#include <stdexcept>
#include <string>

static void fail() { BOOST_THROW_EXCEPTION(std::runtime_error("boom")); }

int main() {
    try {
        fail();
        return 1;
    } catch (const std::runtime_error& e) {
        if (std::string(e.what()) != "boom") return 1;
        boost::source_location loc = boost::get_throw_location(e);
        if (loc.line() == 0 || std::string(loc.file_name()).find("main") == std::string::npos) return 1;
    }

    try {
        boost::throw_exception(std::logic_error("plain"));
        return 1;
    } catch (const boost::exception&) {
    }
    return 0;
}
