#include "Driver.hpp"

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

#if SPDLOG_VERSION > 11500
#  define LOG_ENV "CABIN_LOG"  // NOLINT
static constexpr const char* LOG_ENV_USED = LOG_ENV;
static constexpr const char* LOG_ENV_UNUSED = "SPDLOG_LEVEL";
#else
#  define LOG_ENV  // NOLINT
static constexpr const char* LOG_ENV_USED = "SPDLOG_LEVEL";
static constexpr const char* LOG_ENV_UNUSED = "CABIN_LOG";
#endif

const Cli& getCli() noexcept {
  static const Cli cli =  //
      Cli{ "cabin" }
          .setDesc("A package manager and build system for C++")
          .addOpt(Opt{ "--verbose" }
                      .setShort("-v")
                      .setDesc("Use verbose output (-vv very verbose output)")
                      .setGlobal(true))
          // TODO: assuming -- for long options would be better, also empty
          // long options should be allowed?
          .addOpt(Opt{ "-vv" }
                      .setShort("-vv")
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
                      .setGlobal(true)
                      .setDefault("auto"))
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
          .addSubcmd(REMOVE_CMD)
          .addSubcmd(RUN_CMD)
          .addSubcmd(SEARCH_CMD)
          .addSubcmd(TEST_CMD)
          .addSubcmd(TIDY_CMD)
          .addSubcmd(VERSION_CMD);
  return cli;
}

static std::string colorizeAnyhowError(std::string s) {
  if (s.find("Caused by:") != std::string::npos) {
    replaceAll(s, "Caused by:", Yellow("Caused by:").toErrStr());
  }
  if (s.back() == '\n') {
    s.pop_back();  // remove the last '\n' since Diag::error adds one.
  }
  return s;
}

Result<void, void> run(int argc,
                       char* argv[]) noexcept {  // NOLINT(*-avoid-c-arrays)
  // Set up logger
  spdlog::cfg::load_env_levels(LOG_ENV);
  if (std::getenv(LOG_ENV_UNUSED)) {
    Diag::warn("{} is set but not used. Use {} instead.", LOG_ENV_UNUSED,
               LOG_ENV_USED);
  }

  return getCli()
      .parseArgs(argc, argv)
      .map_err([](const auto& e) { return colorizeAnyhowError(e->what()); })
      .map_err([](std::string e) { Diag::error("{}", std::move(e)); });
}

}  // namespace cabin
