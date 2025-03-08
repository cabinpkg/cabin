#include "Cabin.hpp"

#include "Algos.hpp"
#include "Cli.hpp"
#include "Cmd.hpp"
#include "Diag.hpp"
#include "Rustify/Result.hpp"
#include "TermColor.hpp"

#include <cstdlib>
#include <spdlog/cfg/env.h>
#include <spdlog/version.h>
#include <string>
#include <utility>

namespace cabin {

const Cli&
getCli() noexcept {
  static const Cli cli =  //
      Cli{ "cabin" }
          .setDesc("A package manager and build system for C++")
          .addOpt(
              Opt{ "--verbose" }
                  .setShort("-v")
                  .setDesc("Use verbose output (-vv very verbose output)")
                  .setGlobal(true)
          )
          // TODO: assuming -- for long options would be better, also empty
          // long options should be allowed?
          .addOpt(
              Opt{ "-vv" }
                  .setShort("-vv")
                  .setDesc("Use very verbose output")
                  .setGlobal(true)
                  .setHidden(true)
          )
          .addOpt(
              Opt{ "--quiet" }
                  .setShort("-q")
                  .setDesc("Do not print cabin log messages")
                  .setGlobal(true)
          )
          .addOpt(
              Opt{ "--color" }
                  .setDesc("Coloring: auto, always, never")
                  .setPlaceholder("<WHEN>")
                  .setGlobal(true)
          )
          .addOpt(
              Opt{ "--help" }  //
                  .setShort("-h")
                  .setDesc("Print help")
                  .setGlobal(true)
          )
          .addOpt(
              Opt{ "--version" }
                  .setShort("-V")
                  .setDesc("Print version info and exit")
                  .setGlobal(false)
          )
          .addOpt(
              Opt{ "--list" }  //
                  .setDesc("List all subcommands")
                  .setGlobal(false)
                  .setHidden(true)
          )
          .addSubcmd(ADD_CMD)
          .addSubcmd(BUILD_CMD)
          .addSubcmd(CLEAN_CMD)
          .addSubcmd(FMT_CMD)
          .addSubcmd(HELP_CMD)
          .addSubcmd(INIT_CMD)
          .addSubcmd(LINT_CMD)
          .addSubcmd(NEW_CMD)
          .addSubcmd(REMOVE_CMD)
          .addSubcmd(RUN_CMD)
          .addSubcmd(SEARCH_CMD)
          .addSubcmd(TEST_CMD)
          .addSubcmd(TIDY_CMD)
          .addSubcmd(VERSION_CMD);
  return cli;
}

static std::string
colorizeAnyhowError(std::string s) {
  if (s.find("Caused by:") != std::string::npos) {
    replaceAll(s, "Caused by:", Yellow("Caused by:").toErrStr());
  }
  if (s.back() == '\n') {
    s.pop_back();  // remove the last '\n' since Diag::error adds one.
  }
  return s;
}

static void
warnUnusedLogEnv() {
#if SPDLOG_VERSION > 11500
  if (std::getenv("SPDLOG_LEVEL")) {
    Diag::warn("SPDLOG_LEVEL is set but not used. Use CABIN_LOG instead.");
  }
#else
  if (std::getenv("CABIN_LOG")) {
    Diag::warn("CABIN_LOG is set but not used. Use SPDLOG_LEVEL instead.");
  }
#endif
}

Result<void, void>
cabinMain(int argc, char* argv[]) noexcept {  // NOLINT(*-avoid-c-arrays)
  // Set up logger
  spdlog::cfg::load_env_levels(
#if SPDLOG_VERSION > 11500
      "CABIN_LOG"
#endif
  );
  warnUnusedLogEnv();

  return getCli()
      .parseArgs(argc, argv)
      .map_err([](const auto& e) { return colorizeAnyhowError(e->what()); })
      .map_err([](std::string e) { Diag::error("{}", std::move(e)); });
}

}  // namespace cabin
