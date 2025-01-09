#include "Help.hpp"

#include "../Cli.hpp"

#include <span>
#include <string_view>

namespace cabin {

static Result<void> helpMain(std::span<const std::string_view> args) noexcept;

const Subcmd HELP_CMD =  //
    Subcmd{ "help" }
        .setDesc("Displays help for a cabin subcommand")
        .setArg(Arg{ "COMMAND" }.setRequired(false))
        .setMainFn(helpMain);

static Result<void>
helpMain(const std::span<const std::string_view> args) noexcept {
  return getCli().printHelp(args);
}

}  // namespace cabin
