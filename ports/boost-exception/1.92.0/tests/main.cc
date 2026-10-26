// Attaches and retrieves error_info on a boost::exception, and transports an
// exception through boost::exception_ptr.
#include <boost/exception/all.hpp>
#include <stdexcept>
#include <string>

typedef boost::error_info<struct tag_code, int> code_info;
typedef boost::error_info<struct tag_what, std::string> what_info;

int main() {
    try {
        throw boost::enable_error_info(std::runtime_error("boom")) << code_info(42);
    } catch (boost::exception& e) {
        const int* code = boost::get_error_info<code_info>(e);
        if (!code || *code != 42) return 1;
        if (boost::get_error_info<what_info>(e)) return 1;
        e << what_info("added later");
        const std::string* w = boost::get_error_info<what_info>(e);
        if (!w || *w != "added later") return 1;
        if (boost::diagnostic_information(e).find("boom") == std::string::npos) return 1;
    }

    boost::exception_ptr p;
    try {
        throw boost::enable_current_exception(std::logic_error("stored"));
    } catch (...) {
        p = boost::current_exception();
    }
    if (!p) return 1;
    try {
        boost::rethrow_exception(p);
    } catch (std::logic_error& e) {
        if (std::string(e.what()) != "stored") return 1;
        return 0;
    }
    return 1;
}
