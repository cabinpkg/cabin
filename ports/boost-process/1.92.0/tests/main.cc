// Exercises Boost.Process v2 without relying on any other program: the
// test finds its own executable through ext::exe, starts itself with an
// argument and an added environment variable, reads the child's stdout
// through a pipe, and checks the child's exit code.
#if defined(_WIN32)
// boost/process.hpp includes <windows.h> ahead of Asio's <winsock2.h>.
#define WIN32_LEAN_AND_MEAN
#endif
#include <boost/asio/io_context.hpp>
#include <boost/asio/read.hpp>
#include <boost/asio/readable_pipe.hpp>
#include <boost/process.hpp>
#include <cstdlib>
#include <cstring>
#include <iostream>
#include <string>
#include <vector>

namespace asio = boost::asio;
namespace bp = boost::process;

int main(int argc, char** argv) {
    if (argc == 2 && std::strcmp(argv[1], "child") == 0) {
        const char* value = std::getenv("CABIN_PROBE_VALUE");
        std::cout << "child:" << (value ? value : "unset") << std::flush;
        return 7;
    }

    std::vector<bp::environment::key_value_pair> env;
    {
        auto current = bp::environment::current();
        env.assign(current.begin(), current.end());
    }
    env.push_back("CABIN_PROBE_VALUE=42");

    asio::io_context ctx;
    asio::readable_pipe out(ctx);
    bp::process child(ctx, bp::ext::exe(bp::current_pid()), {"child"},
                      bp::process_stdio{{}, out, {}}, bp::process_environment(env));
    std::string text;
    boost::system::error_code ec;  // ends at end of file
    // Without pidfd (macOS), the process handle's SIGCHLD handler can
    // interrupt this blocking read, which Asio reports instead of retrying.
    do {
        asio::read(out, asio::dynamic_buffer(text), ec);
    } while (ec == boost::system::errc::interrupted);
    const int code = child.wait();
    return text == "child:42" && code == 7 ? 0 : 1;
}
