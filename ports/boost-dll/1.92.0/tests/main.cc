// Exercises Boost.DLL without building a second module: the running
// program's location and binary format sections, a handle to the program
// itself, a typed symbol imported from an already-loaded system C library,
// and the error paths for a missing symbol and a missing library.
#include <boost/dll/library_info.hpp>
#include <boost/dll/runtime_symbol_info.hpp>
#include <boost/dll/shared_library.hpp>

#include <cstddef>
#include <string>
#include <system_error>
#include <vector>

namespace dll = boost::dll;

#if defined(_WIN32)
static const char* const system_library = "kernel32.dll";
static const char* const length_symbol = "lstrlenA";
typedef int length_fn(const char*);
#elif defined(__APPLE__)
static const char* const system_library = "/usr/lib/libSystem.B.dylib";
static const char* const length_symbol = "strlen";
typedef std::size_t length_fn(const char*);
#else
static const char* const system_library = "libc.so.6";  // glibc
static const char* const length_symbol = "strlen";
typedef std::size_t length_fn(const char*);
#endif

int main() {
    const dll::fs::path exe = dll::program_location();
    if (!exe.is_absolute() || !dll::fs::is_regular_file(exe)) return 1;

    dll::library_info info(exe);
    const std::vector<std::string> sections = info.sections();
    bool has_text = false;
    for (std::size_t i = 0; i < sections.size(); ++i) has_text |= sections[i].find("text") != std::string::npos;
    if (!has_text) return 1;

    dll::shared_library self(exe);
    if (!self.is_loaded()) return 1;

    dll::shared_library lib(system_library, dll::load_mode::search_system_folders);
    if (!lib.has(length_symbol) || lib.has("cabin_no_such_symbol")) return 1;
    if (lib.get<length_fn>(length_symbol)("cabin") != 5) return 1;

    std::error_code ec;
    dll::shared_library missing;
    missing.load("cabin-no-such-library", ec, dll::load_mode::append_decorations);
    return ec && !missing.is_loaded() ? 0 : 1;
}
