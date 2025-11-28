#include "Remove.hpp"

#include "Cli.hpp"
#include "Diag.hpp"
#include "Manifest.hpp"

#include <cstdlib>
#include <fmt/ranges.h>
#include <fstream>
#include <rs/result.hpp>
#include <string>
#include <toml.hpp>
#include <toml11/types.hpp>
#include <vector>

namespace cabin {

static rs::Result<void> removeMain(CliArgsView args);

const Subcmd REMOVE_CMD = //
    Subcmd{ "remove" }
        .setDesc("Remove dependencies from cabin.toml")
        .setArg(Arg{ "deps" }
                    .setDesc("Dependencies to remove")
                    .setRequired(true)
                    .setVariadic(true))
        .setMainFn(removeMain);

static rs::Result<void> removeMain(const CliArgsView args) {
  rs_ensure(!args.empty(), "`cabin remove` requires at least one argument");

  std::vector<std::string_view> removedDeps = {};
  const fs::path manifestPath = rs_try(Manifest::findPath());
  auto data = toml::parse<toml::ordered_type_config>(manifestPath);
  auto& deps = data["dependencies"];

  rs_ensure(!deps.is_empty(), "No dependencies to remove");

  for (const std::string& dep : args) {
    if (deps.contains(dep)) {
      deps.as_table().erase(dep);
      removedDeps.push_back(dep);
    } else {
      // manifestPath needs to be converted to string
      // or it adds extra quotes around the path
      Diag::warn("Dependency `{}` not found in {}", dep, manifestPath.string());
    }
  }

  if (!removedDeps.empty()) {
    std::ofstream out(manifestPath);
    out << data;
    Diag::info("Removed", "{} from {}", fmt::join(removedDeps, ", "),
               manifestPath.string());
  }
  return rs::Ok();
}

} // namespace cabin
