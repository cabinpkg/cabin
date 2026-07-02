#include "greeter/greeter.hpp"

#include <fmt/format.h>

namespace greeter {

std::string greeting(const std::string &name) {
    return fmt::format("Hello, {}! (formatted by fmt {})", name, FMT_VERSION);
}

}  // namespace greeter
