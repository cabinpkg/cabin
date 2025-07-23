#include "Fmt.hpp"

#include "Algos.hpp"
#include "BuildConfig.hpp"
#include "Cli.hpp"
#include "Diag.hpp"
#include "Git2/Exception.hpp"
#include "Git2/Repository.hpp"
#include "Manifest.hpp"
#include "Rustify/Result.hpp"

#include <algorithm>
#include <cstdlib>
#include <ranges>
#include <spdlog/spdlog.h>
#include <string>
#include <string_view>
#include <utility>
#include <vector>

namespace cabin {

static Result<void> fmtMain(CliArgsView args);

const Subcmd FMT_CMD =
    Subcmd{ "fmt" }
        .setDesc("Format codes using clang-format")
        .addOpt(Opt{ "--check" }.setDesc("Run clang-format in check mode"))
        .addOpt(Opt{ "--exclude" }
                    .setDesc("Exclude files from formatting")
                    .setPlaceholder("<FILE>"))
        .addOpt(Opt{ "--no-ignore-vcs" }.setDesc(
            "Do not exclude git-ignored files from formatting"))
        .setMainFn(fmtMain);

static std::vector<std::string>
collectFormatTargets(const fs::path& manifestDir,
                     const std::vector<fs::path>& excludes,
                     bool useVcsIgnoreFiles) {
  // Read git repository if exists
  git2::Repository repo = git2::Repository();
  bool hasGitRepo = false;
  if (useVcsIgnoreFiles) {
    try {
      repo.open(manifestDir.string());
      hasGitRepo = true;
    } catch (const git2::Exception& e) {
      spdlog::debug("No git repository found");
    }
  }

  const auto isExcluded = [&](std::string_view path) -> bool {
    return std::ranges::find_if(
               excludes,
               [&](const fs::path& path2) {
                 return fs::relative(path2, manifestDir).string() == path;
               })
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
        spdlog::debug("Ignore: {}", path);
        entry.disable_recursion_pending();
        continue;
      }
    } else if (entry->is_regular_file()) {
      const fs::path path = fs::relative(entry->path(), manifestDir);
      if ((hasGitRepo && repo.isIgnored(path.string()))
          || isExcluded(path.string())) {
        spdlog::debug("Ignore: {}", path.string());
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

static Result<void>
fmtMain(const CliArgsView args) {
  std::vector<fs::path> excludes;
  bool isCheck = false;
  bool useVcsIgnoreFiles = true;
  // Parse args
  for (auto itr = args.begin(); itr != args.end(); ++itr) {
    const std::string_view arg = *itr;

    const auto control = Try(Cli::handleGlobalOpts(itr, args.end(), "fmt"));
    if (control == Cli::Return) {
      return Ok();
    } else if (control == Cli::Continue) {
      continue;
    } else if (arg == "--check") {
      isCheck = true;
    } else if (arg == "--exclude") {
      if (itr + 1 == args.end()) {
        return Subcmd::missingOptArgumentFor(arg);
      }
      excludes.emplace_back(*++itr);
    } else if (arg == "--no-ignore-vcs") {
      useVcsIgnoreFiles = false;
    } else {
      return FMT_CMD.noSuchArg(arg);
    }
  }

  Ensure(commandExists("clang-format"),
         "fmt command requires clang-format; try installing it by:\n"
         "  apt/brew install clang-format");

  const auto manifest = Try(Manifest::tryParse());
  std::vector<std::string> clangFormatArgs{
    "--style=file",
    "--fallback-style=LLVM",
    "-Werror",
  };

  const fs::path projectPath = manifest.path.parent_path();
  const std::vector<std::string> sources =
      collectFormatTargets(projectPath, excludes, useVcsIgnoreFiles);
  if (sources.empty()) {
    Diag::warn("no files to format");
    return Ok();
  }

  if (isVerbose()) {
    clangFormatArgs.emplace_back("--verbose");
  }
  if (isCheck) {
    clangFormatArgs.emplace_back("--dry-run");
  } else {
    clangFormatArgs.emplace_back("-i");
    Diag::info("Formatting", "{}", manifest.package.name);
  }
  clangFormatArgs.insert(clangFormatArgs.end(), sources.begin(), sources.end());

  const char* cabinFmt = std::getenv("CABIN_FMT");
  if (cabinFmt == nullptr) {
    cabinFmt = "clang-format";
  }

  const Command clangFormat = Command(cabinFmt, std::move(clangFormatArgs))
                                  .setWorkingDirectory(projectPath.string());

  const ExitStatus exitStatus = Try(execCmd(clangFormat));
  if (exitStatus.success()) {
    return Ok();
  } else {
    Bail("clang-format {}", exitStatus);
  }
}

}  // namespace cabin
