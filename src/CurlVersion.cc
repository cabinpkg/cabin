#include "CurlVersion.hpp"

#include <fmt/format.h>
#include <string>

namespace curl {

std::string
Version::toString() const {
  std::string str;
  if (data) {
    str += data->version;
    str += " (ssl: ";
    if (data->ssl_version) {
      str += data->ssl_version;
    } else {
      str += "none";
    }
    str += ")";
  }
  return str;
}

};  // namespace curl

auto
fmt::formatter<curl::Version>::format(const curl::Version& v,
                                      format_context& ctx) const
    -> format_context::iterator {
  return formatter<std::string>::format(v.toString(), ctx);
}
