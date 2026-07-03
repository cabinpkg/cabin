#include "netlib/tls.hpp"

#include "netlib/net.hpp"

namespace netlib {

std::string secure_fetch(const std::string& host) { return "TLS " + fetch(host); }

}  // namespace netlib
