#include "Fmt.hpp"

#include "../Algos.hpp"
#include "../BuildConfig.hpp"
#include "../Cli.hpp"
#include "../Git2/Exception.hpp"
#include "../Git2/Repository.hpp"
#include "../Logger.hpp"
#include "../Manifest.hpp"
#include "../Rustify.hpp"

#include <cstdlib>
#include <fstream>
#include <span>
#include <string>

static int fmtMain(std::span<const StringRef> args);

const Subcmd FMT_CMD =
    Subcmd{ "fmt" }
        .setDesc("Format codes using clang-format")
        .addOpt(Opt{ "--check" }.setDesc("Run clang-format in check mode"))
        .setMainFn(fmtMain);

static void
collectFormatTargetFiles(const Path& manifestDir, String& clangFormatArgs) {
  // Read git repository if exists
  git2::Repository repo = git2::Repository();
  bool hasGitRepo = false;
  try {
    repo.open(manifestDir.string());
    hasGitRepo = true;
  } catch (const git2::Exception& e) {
    Logger::debug("No git repository found");
  }

  // Automatically collects format-target files
  for (auto entry = fs::recursive_directory_iterator(manifestDir);
       entry != fs::recursive_directory_iterator(); ++entry) {
    if (entry->is_directory()) {
      const String path = fs::relative(entry->path(), manifestDir).string();
      if (hasGitRepo && repo.isIgnored(path)) {
        Logger::debug("Ignore: ", path);
        entry.disable_recursion_pending();
        continue;
      }
    } else if (entry->is_regular_file()) {
      const Path path = fs::relative(entry->path(), manifestDir);
      if (hasGitRepo && repo.isIgnored(path.string())) {
        Logger::debug("Ignore: ", path.string());
        continue;
      }

      const String ext = path.extension().string();
      if (SOURCE_FILE_EXTS.contains(ext) || HEADER_FILE_EXTS.contains(ext)) {
        clangFormatArgs += " " + path.string();
      }
    }
  }
}

static int
fmtMain(const std::span<const StringRef> args) {
  bool isCheck = false;
  // Parse args
  for (usize i = 0; i < args.size(); ++i) {
    const StringRef arg = args[i];
    HANDLE_GLOBAL_OPTS({ { "fmt" } })

    else if (arg == "--check") {
      isCheck = true;
    }
    else {
      return FMT_CMD.noSuchArg(arg);
    }
  }

  if (!commandExists("clang-format")) {
    Logger::error(
        "fmt command requires clang-format; try installing it by:\n"
        "  apt/brew install clang-format"
    );
    return EXIT_FAILURE;
  }

  const String& packageName = getPackageName();
  String clangFormatArgs = "--style=file --fallback-style=LLVM -Werror";
  if (isVerbose()) {
    clangFormatArgs += " --verbose";
  }
  if (isCheck) {
    clangFormatArgs += " --dry-run";
  } else {
    clangFormatArgs += " -i";
    Logger::info("Formatting", packageName);
  }

  const Path& manifestDir = getManifestPath().parent_path();
  collectFormatTargetFiles(manifestDir, clangFormatArgs);

  const String clangFormat = "cd " + manifestDir.string()
                             + " && ${POAC_FMT:-clang-format} "
                             + clangFormatArgs;
  return execCmd(clangFormat);
}
