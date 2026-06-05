#include <iostream>

// Which branch compiles is decided by Cabin's `[target.'cfg(...)']`
// platform conditions in `cabin.toml`, not by the compiler's own
// macros — so the same source prints the platform Cabin resolved.
int main() {
#if defined(CABIN_ON_WINDOWS)
    std::cout << "Hello from Cabin on Windows\n";
#elif defined(CABIN_ON_UNIX)
    std::cout << "Hello from Cabin on Unix\n";
#else
    std::cout << "Hello from Cabin on an unknown platform\n";
#endif
    return 0;
}
