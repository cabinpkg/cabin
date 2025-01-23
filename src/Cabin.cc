#include "Cabin.hpp"

#include "Algos.hpp"
#include "Cli.hpp"
#include "Cmd.hpp"
#include "Logger.hpp"
#include "Rustify/Result.hpp"
#include "TermColor.hpp"

#include <cstdlib>
#include <exception>
#include <fmt/core.h>
#include <span>
#include <string>
#include <string_view>
#include <utility>
#include <vector>

namespace cabin {

const Cli&
getCli() noexcept {
  static const Cli cli =  //
      Cli{ "cabin" }
          .setDesc("A package manager and build system for C++")
          .addOpt(Opt{ "--verbose" }
                      .setShort("-v")
                      .setDesc("Use verbose output (-vv very verbose output)")
                      .setGlobal(true))
          .addOpt(Opt{ "-vv" }
                      .setDesc("Use very verbose output")
                      .setGlobal(true)
                      .setHidden(true))
          .addOpt(Opt{ "--quiet" }
                      .setShort("-q")
                      .setDesc("Do not print cabin log messages")
                      .setGlobal(true))
          .addOpt(Opt{ "--color" }
                      .setDesc("Coloring: auto, always, never")
                      .setPlaceholder("<WHEN>")
                      .setGlobal(true))
          .addOpt(Opt{ "--help" }  //
                      .setShort("-h")
                      .setDesc("Print help")
                      .setGlobal(true))
          .addOpt(Opt{ "--version" }
                      .setShort("-V")
                      .setDesc("Print version info and exit")
                      .setGlobal(false))
          .addOpt(Opt{ "--list" }  //
                      .setDesc("List all subcommands")
                      .setGlobal(false)
                      .setHidden(true))
          .addSubcmd(ADD_CMD)
          .addSubcmd(BUILD_CMD)
          .addSubcmd(CLEAN_CMD)
          .addSubcmd(FMT_CMD)
          .addSubcmd(HELP_CMD)
          .addSubcmd(INIT_CMD)
          .addSubcmd(LINT_CMD)
          .addSubcmd(NEW_CMD)
          .addSubcmd(RUN_CMD)
          .addSubcmd(SEARCH_CMD)
          .addSubcmd(TEST_CMD)
          .addSubcmd(TIDY_CMD)
          .addSubcmd(VERSION_CMD);
  return cli;
}

static Result<void>
parseArgs(const std::span<char* const> args) noexcept {
  // Parse arguments (options should appear before the subcommand, as the help
  // message shows intuitively)
  // cabin --verbose run --release help --color always --verbose
  // ^^^^^^^^^^^^^^ ^^^^^^^^^^^^^ ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
  // [global]       [run]         [help (under run)]
  for (auto itr = args.begin(); itr != args.end(); ++itr) {
    // Global options
    const auto control = Try(Cli::handleGlobalOpts(itr, args.end()));
    if (control == Cli::Return) {
      return Ok();
    } else if (control == Cli::Continue) {
      continue;
    }
    // else: Fallthrough: current argument wasn't handled

    // Local options
    else if (*itr == "-V"sv || *itr == "--version"sv) {
      const std::vector<std::string_view> remArgs(itr + 1, args.end());
      return versionMain(remArgs);
    } else if (*itr == "--list"sv) {
      fmt::print("{}", getCli().formatAllSubcmds(true));
      return Ok();
    }

    // Subcommands
    else if (getCli().hasSubcmd(*itr)) {
      const std::vector<std::string_view> remArgs(itr + 1, args.end());
      try {
        return getCli().exec(*itr, remArgs);
      } catch (const std::exception& e) {
        Bail(e.what());
      }
    }

    // Unexpected argument
    else {
      return getCli().noSuchArg(*itr);
    }
  }

  return getCli().printHelp({});
}

static std::string
colorizeAnyhowError(std::string s) {
  // `Caused by:` leaves a trailing newline
  if (s.find("Caused by:") != std::string::npos) {
    replaceAll(s, "Caused by:", Yellow("Caused by:").toErrStr());
    replaceAll(s, "\n", "");
  }
  return s;
}

Result<void, void>
cliMain(int argc, char* argv[]) noexcept {  // NOLINT(*-avoid-c-arrays)
  // Drop the first argument (program name)
  const std::span<char* const> args(argv + 1, argv + argc);
  return parseArgs(args)
      .map_err([](const auto& e) { return colorizeAnyhowError(e->what()); })
      .map_err([](std::string e) { logger::error("{}", std::move(e)); });
}

}  // namespace cabin
