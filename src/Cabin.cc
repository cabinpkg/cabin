#include "Cabin.hpp"

#include "Algos.hpp"
#include "Cli.hpp"
#include "Cmd.hpp"
#include "Logger.hpp"
#include "Rustify/Result.hpp"
#include "TermColor.hpp"

#include <string>
#include <utility>

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

static std::string
colorizeAnyhowError(std::string s) {
  if (s.find("Caused by:") != std::string::npos) {
    replaceAll(s, "Caused by:", Yellow("Caused by:").toErrStr());
    // `Caused by:` leaves a trailing newline, FIXME: upstream this
    replaceAll(s, "\n", "");
  }
  return s;
}

Result<void, void>
cabinMain(int argc, char* argv[]) noexcept {  // NOLINT(*-avoid-c-arrays)
  return getCli()
      .parseArgs(argc, argv)
      .map_err([](const auto& e) { return colorizeAnyhowError(e->what()); })
      .map_err([](std::string e) { logger::error("{}", std::move(e)); });
}

}  // namespace cabin
