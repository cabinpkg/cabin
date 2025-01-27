#include "Remove.hpp"

#include "../Cli.hpp"
#include "../Logger.hpp"
#include "../Manifest.hpp"
#include "../Rustify/Result.hpp"

#include <cstdlib>
#include <fmt/ranges.h>
#include <fstream>
#include <string>
#include <toml.hpp>
#include <toml11/types.hpp>
#include <vector>

namespace cabin {

static Result<void> removeMain(CliArgsView args);

const Subcmd REMOVE_CMD =  //
    Subcmd{ "remove" }
        .setDesc("Remove dependencies from cabin.toml")
        .setArg(Arg{ "deps" }
                    .setDesc("Dependencies to remove")
                    .setRequired(true)
                    .setVariadic(true))
        .setMainFn(removeMain);

static Result<void>
removeMain(const CliArgsView args) {
  Ensure(!args.empty(), "`cabin remove` requires at least one argument");

  std::vector<std::string_view> removedDeps = {};
  const fs::path manifestPath = Try(findManifest());
  auto data = toml::parse<toml::ordered_type_config>(manifestPath);
  auto& deps = data["dependencies"];

  Ensure(!deps.is_empty(), "No dependencies to remove");

  for (const std::string& dep : args) {
    if (deps.contains(dep)) {
      deps.as_table().erase(dep);
      removedDeps.push_back(dep);
    } else {
      // manifestPath needs to be converted to string
      // or it adds extra quotes around the path
      logger::warn(
          "Dependency `{}` not found in {}", dep, manifestPath.string()
      );
    }
  }

  if (!removedDeps.empty()) {
    std::ofstream out(manifestPath);
    out << data;
    logger::info(
        "Removed", "{} from {}", fmt::join(removedDeps, ", "),
        manifestPath.string()
    );
  }
  return Ok();
}

}  // namespace cabin
