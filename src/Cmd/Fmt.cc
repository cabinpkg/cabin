#include "Fmt.hpp"

#include "../Algos.hpp"
#include "../BuildConfig.hpp"
#include "../Cli.hpp"
#include "../Git2/Exception.hpp"
#include "../Git2/Repository.hpp"
#include "../Logger.hpp"
#include "../Manifest.hpp"
#include "../Rustify.hpp"

#include <algorithm>
#include <cstdlib>
#include <ranges>
#include <span>
#include <string>
#include <string_view>
#include <utility>
#include <vector>

static int fmtMain(std::span<const std::string_view> args);

const Subcmd FMT_CMD =
    Subcmd{ "fmt" }
        .setDesc("Format codes using clang-format")
        .addOpt(Opt{ "--check" }.setDesc("Run clang-format in check mode"))
        .addOpt(Opt{ "--exclude" }
                    .setDesc("Exclude files from formatting")
                    .setPlaceholder("<FILE>"))
        .setMainFn(fmtMain);

static std::vector<std::string>
collectFormatTargets(
    const fs::path& manifestDir, const std::vector<fs::path>& excludes
) {
  // Read git repository if exists
  git2::Repository repo = git2::Repository();
  bool hasGitRepo = false;
  try {
    repo.open(manifestDir.string());
    hasGitRepo = true;
  } catch (const git2::Exception& e) {
    logger::debug("No git repository found");
  }

  const auto isExcluded = [&](std::string_view path) -> bool {
    return std::ranges::find_if(
               excludes,
               [&](const fs::path& path2) {
                 return fs::relative(path2, manifestDir).string() == path;
               }
           )
           != excludes.end();
  };

  // Automatically collects format-target files
  std::vector<std::string> sources;
  for (auto entry = fs::recursive_directory_iterator(manifestDir);
       entry != fs::recursive_directory_iterator(); ++entry) {
    if (entry->is_directory()) {
      const std::string path =
          fs::relative(entry->path(), manifestDir).string();
      if ((hasGitRepo && repo.isIgnored(path)) || isExcluded(path)) {
        logger::debug("Ignore: {}", path);
        entry.disable_recursion_pending();
        continue;
      }
    } else if (entry->is_regular_file()) {
      const fs::path path = fs::relative(entry->path(), manifestDir);
      if ((hasGitRepo && repo.isIgnored(path.string()))
          || isExcluded(path.string())) {
        logger::debug("Ignore: {}", path.string());
        continue;
      }

      const std::string ext = path.extension().string();
      if (SOURCE_FILE_EXTS.contains(ext) || HEADER_FILE_EXTS.contains(ext)) {
        sources.push_back(path.string());
      }
    }
  }
  return sources;
}

static int
fmtMain(const std::span<const std::string_view> args) {
  std::vector<fs::path> excludes;
  bool isCheck = false;
  // Parse args
  for (auto itr = args.begin(); itr != args.end(); ++itr) {
    if (const auto res = Cli::handleGlobalOpts(itr, args.end(), "fmt")) {
      if (res.value() == Cli::CONTINUE) {
        continue;
      } else {
        return res.value();
      }
    } else if (*itr == "--check") {
      isCheck = true;
    } else if (*itr == "--exclude") {
      if (itr + 1 == args.end()) {
        return Subcmd::missingArgumentForOpt(*itr);
      }

      excludes.emplace_back(*++itr);
    } else {
      return FMT_CMD.noSuchArg(*itr);
    }
  }

  if (!commandExists("clang-format")) {
    logger::error(
        "fmt command requires clang-format; try installing it by:\n"
        "  apt/brew install clang-format"
    );
    return EXIT_FAILURE;
  }

  const auto manifestRes = Manifest::tryParse();
  if (manifestRes.is_err()) {
    throw CabinError(manifestRes.unwrap_err()->what());
  }
  const auto& manifest = manifestRes.unwrap();
  std::vector<std::string> clangFormatArgs{
    "--style=file",
    "--fallback-style=LLVM",
    "-Werror",
  };

  const fs::path projectPath = manifest.path.parent_path();
  const std::vector<std::string> sources =
      collectFormatTargets(projectPath, excludes);
  if (sources.empty()) {
    logger::warn("no files to format");
    return EXIT_SUCCESS;
  }

  if (isVerbose()) {
    clangFormatArgs.emplace_back("--verbose");
  }
  if (isCheck) {
    clangFormatArgs.emplace_back("--dry-run");
  } else {
    clangFormatArgs.emplace_back("-i");
    logger::info("Formatting", "{}", manifest.package.name);
  }
  clangFormatArgs.insert(clangFormatArgs.end(), sources.begin(), sources.end());

  const char* cabinFmt = std::getenv("CABIN_FMT");
  if (cabinFmt == nullptr) {
    cabinFmt = "clang-format";
  }

  const Command clangFormat = Command(cabinFmt, std::move(clangFormatArgs))
                                  .setWorkingDirectory(projectPath.string());

  return execCmd(clangFormat);
}
