#include "Add.hpp"

#include "../Cli.hpp"
#include "../Diag.hpp"
#include "../Manifest.hpp"
#include "../Rustify/Result.hpp"

#include <cstdlib>
#include <fmt/std.h>
#include <fstream>
#include <functional>
#include <string>
#include <string_view>
#include <toml.hpp>
#include <unordered_map>
#include <unordered_set>

namespace cabin {

static Result<void> addMain(CliArgsView args);

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

static Result<void>
handleNextArg(
    CliArgsView::iterator& itr, const CliArgsView::iterator& end,
    std::string& arg
) {
  ++itr;
  if (itr == end) {
    return Subcmd::missingOptArgumentFor(*--itr);
  }
  arg = std::string(*itr);
  return Ok();
}

static void
handleDependency(
    std::unordered_set<std::string_view>& newDeps, const std::string_view dep
) {
  if (newDeps.contains(dep)) {
    Diag::warn("The dependency `{}` is already in the cabin.toml", dep);
    return;
  }
  newDeps.insert(dep);
}

static std::string
getDependencyGitUrl(const std::string_view dep) {
  if (dep.find("://") == std::string_view::npos) {
    // Check if at least in "user/repo" format.
    if (dep.find('/') == std::string_view::npos) {
      Diag::error("Invalid dependency: {}", dep);
      return "";
    }

    return "https://github.com/" + std::string(dep) + ".git";
  }
  return std::string(dep);
}

static std::string
getDependencyName(const std::string_view dep) {
  using std::string_view_literals::operator""sv;

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

static Result<void>
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
    Ensure(
        !version.empty(),
        "The `--version` option is required for system dependencies"
    );
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
  const fs::path manifestPath = Try(Manifest::findPath());
  auto data = toml::parse<toml::ordered_type_config>(manifestPath);

  // Check if the dependencies table exists, if not create it.
  if (data["dependencies"].is_empty()) {
    data["dependencies"] = toml::table{};
  }
  auto& deps = data["dependencies"];

  for (const auto& dep : newDeps) {
    if (!isSystemDependency) {
      const std::string gitUrl = getDependencyGitUrl(dep);
      const std::string depName = getDependencyName(dep);

      Ensure(
          !gitUrl.empty() && !depName.empty(),
          "git URL or dependency name must not be empty: {}", dep
      );

      deps[depName] = depData;
      deps[depName]["git"] = gitUrl;
    } else {
      deps[std::string(dep)] = depData;
    }
  }

  std::ofstream ofs(manifestPath);
  ofs << data;

  Diag::info("Added", "to the cabin.toml");
  return Ok();
}

static Result<void>
addMain(const CliArgsView args) {
  Ensure(!args.empty(), "No dependencies to add");

  std::unordered_set<std::string_view> newDeps = {};

  bool isSystemDependency = false;
  std::string version;  // Only used with system-dependencies

  std::string tag;
  std::string rev;
  std::string branch;

  // clang-format off
  std::unordered_map<
    std::string_view,
    std::function<Result<void>(decltype(args)::iterator&, decltype(args)::iterator)>
  >
  handlers = {
    {
      "--sys", [&](auto&, auto) {
        isSystemDependency = true;
        return Ok();
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
    const auto control = Try(Cli::handleGlobalOpts(itr, args.end(), "add"));
    if (control == Cli::Return) {
      return Ok();
    } else if (control == Cli::Continue) {
      continue;
    } else {
      if (handlers.contains(*itr)) {
        Try(handlers.at(*itr)(itr, args.end()));
      } else {
        handleDependency(newDeps, *itr);
      }
    }
  }

  return addDependencyToManifest(
      newDeps, isSystemDependency, version, tag, rev, branch
  );
}

}  // namespace cabin
