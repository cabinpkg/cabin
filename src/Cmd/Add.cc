#include "Add.hpp"

#include "../Cli.hpp"
#include "../Logger.hpp"
#include "../Manifest.hpp"

#include <cstdlib>
#include <fmt/std.h>
#include <fstream>
#include <functional>
#include <optional>
#include <span>
#include <string>
#include <string_view>
#include <toml.hpp>
#include <unordered_map>
#include <unordered_set>

static int addMain(std::span<const std::string_view> args);

const Subcmd ADD_CMD =
    Subcmd{ "add" }
        .setDesc("Add dependencies to cabin.toml")
        .setArg(Arg{ "args" }
                    .setDesc("Dependencies to add")
                    .setRequired(true)
                    .setVariadic(true))
        .addOpt(Opt{ "--sys" }.setDesc("Use system dependency"))
        .addOpt(Opt{ "--version" }.setDesc(
            "Dependency version (Only used with system-dependencies)"
        ))
        .addOpt(
            Opt{ "--tag" }.setDesc("Specify a git tag").setPlaceholder("<TAG>")
        )
        .addOpt(Opt{ "--rev" }
                    .setDesc("Specify a git revision")
                    .setPlaceholder("<REVISION>"))
        .addOpt(Opt{ "--branch" }
                    .setDesc("Specify a branch of the git repository")
                    .setPlaceholder("<BRANCH_NAME>"))
        .setMainFn(addMain);

static std::optional<int>
handleNextArg(
    std::span<const std::string_view>::iterator& itr,
    const std::span<const std::string_view>::iterator& end, std::string& arg
) {
  ++itr;
  if (itr == end) {
    return Subcmd::missingArgumentForOpt(*--itr);
  }
  arg = std::string(*itr);
  return std::nullopt;
}

static void
handleDependency(
    std::unordered_set<std::string_view>& newDeps, const std::string_view dep
) {
  if (newDeps.contains(dep)) {
    logger::warn("The dependency `{}` is already in the cabin.toml", dep);
    return;
  }
  newDeps.insert(dep);
}

static std::string
getDependencyGitUrl(const std::string_view dep) {
  if (dep.find("://") == std::string_view::npos) {
    // Check if at least in "user/repo" format.
    if (dep.find('/') == std::string_view::npos) {
      logger::error("Invalid dependency: {}", dep);
      return "";
    }

    return "https://github.com/" + std::string(dep) + ".git";
  }
  return std::string(dep);
}

static std::string
getDependencyName(const std::string_view dep) {
  std::string name;
  if (dep.find("://") == std::string_view::npos) {
    name = dep.substr(dep.find_last_of('/') + 1);
  } else {
    name = dep.substr(
        dep.find_last_of('/') + 1, dep.find(".git") - dep.find_last_of('/') - 1
    );
  }

  // Remove trailing '.git' if it exists.
  if (name.ends_with(".git")) {
    name = name.substr(0, name.size() - ".git"sv.size());
  }

  return name;
}

static int
addDependencyToManifest(
    const std::unordered_set<std::string_view>& newDeps,
    bool isSystemDependency, std::string& version, std::string& tag,
    std::string& rev, std::string& branch
) {
  toml::value depData = toml::table{};
  // Set the formatting for the dependency data table to be on a single line.
  // e.g. dep = { git = "https://github.com/user/repo.git", tag = "v1.0.0" }
  depData.as_table_fmt().fmt = toml::table_format::oneline;

  if (isSystemDependency) {
    if (version.empty()) {
      logger::error("The `--version` option is required for system dependencies"
      );
      return EXIT_FAILURE;
    }
    depData["version"] = version;
    depData["system"] = true;
  } else {
    if (!tag.empty()) {
      depData["tag"] = tag;
    }
    if (!rev.empty()) {
      depData["rev"] = rev;
    }
    if (!branch.empty()) {
      depData["branch"] = branch;
    }
  }

  // Keep the order of the tables.
  const Result<fs::path> manifestPath = findManifest();
  if (manifestPath.is_err()) {
    throw CabinError(manifestPath.unwrap_err()->what());
    return EXIT_FAILURE;
  }
  auto data = toml::parse<toml::ordered_type_config>(manifestPath.unwrap());

  // Check if the dependencies table exists, if not create it.
  if (data["dependencies"].is_empty()) {
    data["dependencies"] = toml::table{};
  }
  auto& deps = data["dependencies"];

  for (const auto& dep : newDeps) {
    if (!isSystemDependency) {
      const std::string gitUrl = getDependencyGitUrl(dep);
      const std::string depName = getDependencyName(dep);

      if (gitUrl.empty() || depName.empty()) {
        return EXIT_FAILURE;
      }

      deps[depName] = depData;
      deps[depName]["git"] = gitUrl;
    } else {
      deps[std::string(dep)] = depData;
    }
  }

  std::ofstream ofs(manifestPath.unwrap());
  ofs << data;

  logger::info("Added", "to the cabin.toml");
  return EXIT_SUCCESS;
}

static int
addMain(const std::span<const std::string_view> args) {
  if (args.empty()) {
    logger::error("No dependencies to add");
    return EXIT_FAILURE;
  }

  std::unordered_set<std::string_view> newDeps = {};

  bool isSystemDependency = false;
  std::string version;  // Only used with system-dependencies

  std::string tag;
  std::string rev;
  std::string branch;

  // clang-format off
  std::unordered_map<
    std::string_view,
    std::function<std::optional<int>(decltype(args)::iterator&, decltype(args)::iterator)>
  >
  handlers = {
    {
      "--sys", [&](auto&, auto) {
        isSystemDependency = true;
        return std::nullopt;
      }
    },
    {
      "--version", [&](auto& itr, const auto end) {
        return handleNextArg(itr, end, version);
      }
    },
    {
      "-v", [&](auto& itr, const auto end) {
        return handleNextArg(itr, end, version);
      }
    },
    {
      "--tag", [&](auto& itr, const auto end) {
        return handleNextArg(itr, end, tag);
      }
    },
    {
      "--rev", [&](auto& itr, const auto end) {
        return handleNextArg(itr, end, rev);
      }
    },
    {
      "--branch", [&](auto& itr, const auto end) {
        return handleNextArg(itr, end, branch);
      }
    },
  };
  // clang-format on

  for (auto itr = args.begin(); itr != args.end(); ++itr) {
    if (const auto res = Cli::handleGlobalOpts(itr, args.end(), "add")) {
      if (res.value() == Cli::CONTINUE) {
        continue;
      } else {
        return res.value();
      }
    } else {
      auto handler = handlers.find(*itr);
      if (handler != handlers.end()) {
        if (auto res = handler->second(itr, args.end()); res.has_value()) {
          return res.value();
        }
      } else {
        handleDependency(newDeps, *itr);
      }
    }
  }

  return addDependencyToManifest(
      newDeps, isSystemDependency, version, tag, rev, branch
  );
}
