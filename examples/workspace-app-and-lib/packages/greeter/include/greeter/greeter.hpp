#pragma once

#include <string>

namespace greeter {

// Formats the greeting with the external {fmt} port; the fmt archive
// and headers reach `app` transitively through this library edge.
std::string greeting(const std::string &name);

}  // namespace greeter
