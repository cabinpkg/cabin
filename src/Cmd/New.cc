#include "New.hpp"

#include "Algos.hpp"
#include "Cli.hpp"
#include "Common.hpp"
#include "Diag.hpp"
#include "Git2.hpp"
#include "Manifest.hpp"

#include <cstdlib>
#include <format>
#include <fstream>
#include <rs/result.hpp>
#include <spdlog/spdlog.h>
#include <string>
#include <string_view>
#include <vector>

namespace cabin {

static Result<void> newMain(CliArgsView args);

const Subcmd NEW_CMD = //
    Subcmd{ "new" }
        .setDesc("Create a new cabin project")
        .addOpt(OPT_BIN)
        .addOpt(OPT_LIB)
        .setArg(Arg{ "name" })
        .setMainFn(newMain);

static constexpr std::string_view MAIN_CC = R"(#include <print>

int main(int argc, char* argv[]) {
  std::println("Hello, world!");
  return 0;
}
)";

static std::string toNamespaceName(std::string_view projectName) {
  return replaceAll(std::string(projectName), "-", "_");
}

static std::string getAuthor() noexcept {
  try {
    git2::Config config = git2::Config();
    config.openDefault();
    return std::format("{} <{}>", config.getString("user.name"),
                       config.getString("user.email"));
  } catch (const git2::Exception& e) {
    spdlog::debug("{}", e.what());
    return "";
  }
}

std::string createCabinToml(const std::string_view projectName) noexcept {
  const std::string author = getAuthor();
  return std::format(R"([package]
name = "{}"
version = "0.1.0"
authors = ["{}"]
edition = "23"
)",
                     projectName, author);
}

static std::string getHeader(const std::string_view projectName) noexcept {
  const std::string projectNameUpper = toMacroName(projectName);
  const std::string ns = toNamespaceName(projectName);
  return std::format(R"(#ifndef {0}_HPP
#define {0}_HPP

namespace {1} {{
void hello_world();
}}  // namespace {1}

#endif  // !{0}_HPP
)",
                     projectNameUpper, ns);
}

struct FileTemplate {
  fs::path path;
  std::string contents;
};

static Result<void> writeToFile(const fs::path& fpath, const std::string& text,
                                const bool skipIfExists = false) {
  if (fs::exists(fpath)) {
    if (skipIfExists) {
      return Ok();
    }
    Bail("refusing to overwrite `{}`; file already exists", fpath.string());
  }

  std::ofstream ofs(fpath, std::ios::trunc);
  Ensure(ofs.is_open(), "opening `{}` failed", fpath.string());
  ofs << text;
  Ensure(static_cast<bool>(ofs), "writing `{}` failed", fpath.string());
  return Ok();
}

Result<void> createProjectFiles(const bool isBin, const fs::path& root,
                                const std::string_view projectName,
                                const bool skipExisting) {
  std::vector<FileTemplate> templates;
  if (isBin) {
    fs::create_directories(root / "src");
    templates.push_back(FileTemplate{
        .path = root / "cabin.toml",
        .contents = createCabinToml(projectName),
    });
    templates.push_back(
        FileTemplate{ .path = root / ".gitignore", .contents = "/cabin-out" });
    templates.push_back(FileTemplate{ .path = root / "src" / "main.cc",
                                      .contents = std::string(MAIN_CC) });
  } else {
    const fs::path includeDir = root / "include" / projectName;
    const fs::path libPath = (root / "lib" / projectName).string() + ".cc";
    fs::create_directories(includeDir);
    fs::create_directories(root / "lib");
    const std::string ns = toNamespaceName(projectName);
    templates.push_back(FileTemplate{
        .path = root / "cabin.toml",
        .contents = createCabinToml(projectName),
    });
    templates.push_back(FileTemplate{ .path = root / ".gitignore",
                                      .contents = "/cabin-out\ncabin.lock" });
    templates.push_back(FileTemplate{
        .path = (includeDir / projectName).string() + ".hpp",
        .contents = getHeader(projectName),
    });
    const std::string libImpl = std::format(
        R"(#include "{0}/{0}.hpp"
#include <print>

namespace {1} {{
void hello_world() {{
  std::println("Hello, world from {0}!");
}}
}}  // namespace {1}
)",
        projectName, ns);
    templates.push_back(FileTemplate{ .path = libPath, .contents = libImpl });
  }

  for (const FileTemplate& file : templates) {
    Try(writeToFile(file.path, file.contents, skipExisting));
  }

  Diag::info("Created", "{} `{}` package",
             isBin ? "binary (application)" : "library", projectName);
  return Ok();
}

static Result<void> newMain(const CliArgsView args) {
  // Parse args
  bool isBin = true;
  std::string packageName;
  for (auto itr = args.begin(); itr != args.end(); ++itr) {
    const std::string_view arg = *itr;

    const auto control = Try(Cli::handleGlobalOpts(itr, args.end(), "new"));
    if (control == Cli::Return) {
      return Ok();
    } else if (control == Cli::Continue) {
      continue;
    } else if (matchesAny(arg, { "-b", "--bin" })) {
      isBin = true;
    } else if (matchesAny(arg, { "-l", "--lib" })) {
      isBin = false;
    } else if (packageName.empty()) {
      packageName = arg;
    } else {
      return NEW_CMD.noSuchArg(arg);
    }
  }

  Try(validatePackageName(packageName));
  Ensure(!fs::exists(packageName), "directory `{}` already exists",
         packageName);

  Try(createProjectFiles(isBin, fs::path(packageName), packageName));
  git2::Repository().init(packageName);
  return Ok();
}

} // namespace cabin
