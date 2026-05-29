#include "greet/greet.hpp"

#include <string>

namespace greet {

std::string hello(std::string_view name) {
    std::string out = "Hello, ";
    out.append(name);
    out.push_back('!');
    return out;
}

}  // namespace greet
