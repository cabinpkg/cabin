#ifndef POAC_CMD_SEARCH_HPP_
#define POAC_CMD_SEARCH_HPP_

// std
#include <iostream>
#include <sstream>

// external
#include <boost/algorithm/string.hpp>
#include <boost/property_tree/json_parser.hpp>
#include <boost/property_tree/ptree.hpp>
#include <spdlog/spdlog.h> // NOLINT(build/include_order)
#include <structopt/app.hpp>

// internal
#include "poac/config.hpp"
#include "poac/poac.hpp"
#include "poac/util/net.hpp"
#include "poac/util/pretty.hpp"
#include "poac/util/termcolor2/termcolor2.hpp"
#include "poac/util/verbosity.hpp"

namespace poac::cmd::search {

struct Options : structopt::sub_command {
  /// Package name to search
  String package_name;
};

using NotFound = Error<"No packages found for `{}`", String>;

[[nodiscard]] Result<void>
search(const Options& opts) {
  const boost::property_tree::ptree pt =
      Try(util::net::api::search(opts.package_name, 20).map_err(to_anyhow));
  if (util::verbosity::is_verbose()) {
    boost::property_tree::json_parser::write_json(std::cout, pt);
  }

  const auto children = pt.get_child("data");
  if (children.empty()) {
    return Err<NotFound>(opts.package_name);
  }
  for (const boost::property_tree::ptree::value_type& child : children) {
    const boost::property_tree::ptree& hits = child.second;
    const auto package = format(
        "{} = \"{}\"", hits.get<String>("name"), hits.get<String>("version")
    );

    String description = hits.get<String>("description");
    description = util::pretty::clip_string(description, 100);
    // If util::pretty::clip_string clips last \n, \n should be removed
    description.erase(
        std::remove(description.begin(), description.end(), '\n'),
        description.end()
    );

    spdlog::info("{:<40}# {}", package, description);
  }
  return Ok();
}

[[nodiscard]] Result<void>
exec(const Options& opts) {
  return search(opts);
}

} // namespace poac::cmd::search

STRUCTOPT(poac::cmd::search::Options, package_name);

#endif // POAC_CMD_SEARCH_HPP_
