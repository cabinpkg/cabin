// Exercises Boost.System: error_code/error_condition from errc, categories and messages,
// system_error, interop with std::error_code, and result<T>.
#include <boost/system.hpp>
#include <system_error>

int main() {
    namespace sys = boost::system;
    sys::error_code ok;
    if (ok || ok.value() != 0) return 1;

    sys::error_code ec = sys::errc::make_error_code(sys::errc::invalid_argument);
    if (!ec || ec != sys::errc::invalid_argument) return 1;
    if (ec.category() != sys::generic_category() || ec.message().empty()) return 1;
    if (ec.default_error_condition() != sys::errc::invalid_argument) return 1;

    std::error_code std_ec = ec;
    if (std_ec != std::errc::invalid_argument) return 1;

    try {
        throw sys::system_error(ec, "parse");
    } catch (const sys::system_error& e) {
        if (e.code() != ec) return 1;
    }

    sys::result<int> good = 5;
    sys::result<int> bad = ec;
    if (!good || *good != 5 || bad || bad.error() != ec) return 1;
    return 0;
}
