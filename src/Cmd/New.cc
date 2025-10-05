#include "New.hpp"

#include "Algos.hpp"
#include "Cli.hpp"
#include "Common.hpp"
#include "Diag.hpp"
#include "Git2.hpp"
#include "Manifest.hpp"
#include "Rustify/Result.hpp"

#include <cstdlib>
#include <fmt/core.h>
#include <fmt/format.h>
#include <fstream>
#include <spdlog/spdlog.h>
#include <string>
#include <string_view>

namespace cabin {

static Result<void> newMain(CliArgsView args);

const Subcmd NEW_CMD = //
    Subcmd{ "new" }
        .setDesc("Create a new cabin project")
        .addOpt(OPT_BIN)
        .addOpt(OPT_LIB)
        .addOpt(OPT_MODULES)
        .setArg(Arg{ "name" })
        .setMainFn(newMain);

static constexpr std::string_view MAIN_CC =
    "#include <iostream>\n\n"
    "int main() {\n"
    "    std::cout << \"Hello, world!\" << std::endl;\n"
    "    return 0;\n"
    "}\n";

static constexpr std::string_view MAIN_MODULES_CC =
    "import std;\n\n"
    "int main() {\n"
    "    std::println(\"Hello, world!\");\n"
    "    return 0;\n"
    "}\n";

static std::string getAuthor() noexcept {
  try {
    git2::Config config = git2::Config();
    config.openDefault();
    return fmt::format("{} <{}>", config.getString("user.name"),
                       config.getString("user.email"));
  } catch (const git2::Exception& e) {
    spdlog::debug("{}", e.what());
    return "";
  }
}

std::string createCabinToml(const std::string_view projectName,
                            const bool useModules) noexcept {
  return fmt::format("[package]\n"
                     "name = \"{}\"\n"
                     "version = \"0.1.0\"\n"
                     "authors = [\"{}\"]\n"
                     "edition = \"23\"\n{}",
                     projectName, getAuthor(),
                     useModules ? "modules = true\n" : "");
}

static std::string getHeader(const std::string_view projectName) noexcept {
  const std::string projectNameUpper = toMacroName(projectName);
  return fmt::format("#ifndef {}_HPP\n"
                     "#define {}_HPP\n\n"
                     "namespace {} {{\n}}\n\n"
                     "#endif  // !{}_HPP\n",
                     projectNameUpper, projectNameUpper, projectName,
                     projectNameUpper);
}

static std::string
getModuleInterface(const std::string_view projectName) noexcept {
  return fmt::format("export module {};\n\n"
                     "export namespace {} {{\n}}\n",
                     projectName, projectName);
}

static Result<void> writeToFile(std::ofstream& ofs, const fs::path& fpath,
                                const std::string_view text) {
  ofs.open(fpath);
  if (ofs.is_open()) {
    ofs << text;
  }
  ofs.close();

  if (!ofs) {
    Bail("writing `{}` failed", fpath.string());
  }
  ofs.clear();
  return Ok();
}

static Result<void> createTemplateFiles(const bool isBin, const bool useModules,
                                        const std::string_view projectName) {
  std::ofstream ofs;

  if (isBin) {
    fs::create_directories(projectName / fs::path("src"));
    Try(writeToFile(ofs, projectName / fs::path("cabin.toml"),
                    createCabinToml(projectName, useModules)));
    Try(writeToFile(ofs, projectName / fs::path(".gitignore"), "/cabin-out"));
    Try(writeToFile(ofs, projectName / fs::path("src") / "main.cc",
                    useModules ? MAIN_MODULES_CC : MAIN_CC));
    Diag::info("Created", "binary (application) `{}` package", projectName);
  } else {
    if (useModules) {
      fs::create_directories(projectName / fs::path("src"));
      Try(writeToFile(
          ofs, (projectName / fs::path("src") / projectName).string() + ".cppm",
          getModuleInterface(projectName)));
    } else {
      fs::create_directories(projectName / fs::path("include") / projectName);
      Try(writeToFile(ofs, projectName / fs::path("cabin.toml"),
                      createCabinToml(projectName, useModules)));
      Try(writeToFile(ofs, projectName / fs::path(".gitignore"),
                      "/cabin-out\ncabin.lock"));
      Try(writeToFile(
          ofs,
          (projectName / fs::path("include") / projectName / projectName)
                  .string()
              + ".hpp",
          getHeader(projectName)));
    }
    Diag::info("Created", "library `{}` package", projectName);
  }
  return Ok();
}

static Result<void> newMain(const CliArgsView args) {
  // Parse args
  bool isBin = true;
  bool useModules = false;
  std::string packageName;
  for (auto itr = args.begin(); itr != args.end(); ++itr) {
    const std::string_view arg = *itr;

    const auto control = Try(Cli::handleGlobalOpts(itr, args.end(), "new"));
    if (control == Cli::Return) {
      return Ok();
    } else if (control == Cli::Continue) {
      continue;
    } else if (arg == "-b" || arg == "--bin") {
      isBin = true;
    } else if (arg == "-l" || arg == "--lib") {
      isBin = false;
    } else if (arg == "-m" || arg == "--modules") {
      useModules = true;
    } else if (packageName.empty()) {
      packageName = arg;
    } else {
      return NEW_CMD.noSuchArg(arg);
    }
  }

  Try(validatePackageName(packageName));
  Ensure(!fs::exists(packageName), "directory `{}` already exists",
         packageName);

  Try(createTemplateFiles(isBin, useModules, packageName));
  git2::Repository().init(packageName);
  return Ok();
}

} // namespace cabin
