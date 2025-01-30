#pragma once

#include <curl/curl.h>
#include <fmt/format.h>
#include <string>

namespace curl {

struct Version {
  curl_version_info_data* data;

  Version() : data(curl_version_info(CURLVERSION_NOW)) {}

  std::string toString() const;
};

};  // namespace curl

template <>
struct fmt::formatter<curl::Version> : formatter<std::string> {
  auto format(const curl::Version& v, format_context& ctx) const
      -> format_context::iterator;
};
