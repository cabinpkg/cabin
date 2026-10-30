#pragma once

#include <string>

namespace greeter {

// Formats the greeting with the external {fmt} port.  Only greeter.cc
// includes fmt's headers, so the edge is private: the fmt archive still
// reaches `app`'s link, its headers do not reach `app`'s compiles.
std::string greeting(const std::string &name);

}  // namespace greeter
