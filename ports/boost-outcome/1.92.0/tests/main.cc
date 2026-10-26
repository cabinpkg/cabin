// Exercises Boost.Outcome: result<T> success and failure states, error propagation with
// BOOST_OUTCOME_TRY, and outcome<T> carrying an exception.
#include <boost/outcome.hpp>
#include <stdexcept>
#include <string>

namespace outcome = BOOST_OUTCOME_V2_NAMESPACE;

static outcome::result<int> parse(const std::string& s) {
    if (s.empty() || s.find_first_not_of("0123456789") != std::string::npos)
        return boost::system::errc::invalid_argument;
    return std::stoi(s);
}

static outcome::result<int> sum(const std::string& a, const std::string& b) {
    BOOST_OUTCOME_TRY(auto x, parse(a));
    BOOST_OUTCOME_TRY(auto y, parse(b));
    return x + y;
}

int main() {
    outcome::result<int> ok = sum("12", "30");
    if (!ok || ok.value() != 42) return 1;

    outcome::result<int> bad = sum("12", "x");
    if (bad || bad.error() != boost::system::errc::invalid_argument) return 1;

    outcome::outcome<int> ex(std::make_exception_ptr(std::runtime_error("boom")));
    if (ex.has_value() || !ex.has_exception()) return 1;
    try {
        ex.value();
        return 1;
    } catch (const std::runtime_error& e) {
        if (std::string(e.what()) != "boom") return 1;
    }
    return 0;
}
