#include "Cli.hpp"

#include "Algos.hpp"
#include "Diag.hpp"
#include "Rustify/Result.hpp"
#include "TermColor.hpp"

#include <algorithm>
#include <cassert>
#include <cstdio>
#include <cstdlib>
#include <fmt/core.h>
#include <functional>
#include <span>
#include <string>
#include <string_view>
#include <unordered_map>
#include <utility>
#include <vector>

namespace cabin {

static constinit const std::string_view PADDING = "  ";

static std::string
formatLeft(const std::size_t offset, const std::string_view left) noexcept {
  return fmt::format("{}{:<{}}", PADDING, left, offset + PADDING.size());
}

static std::string
formatHeader(const std::string_view header) noexcept {
  return fmt::format("{}\n", Bold(Green(header)).toStr());
}

static std::string
formatUsage(
    const std::string_view name, const std::string_view cmd,
    const std::string_view usage
) noexcept {
  std::string str = Bold(Green("Usage: ")).toStr();
  str += Bold(Cyan(name)).toStr();
  str += ' ';
  if (!cmd.empty()) {
    str += Bold(Cyan(cmd)).toStr();
    str += ' ';
  }
  str += Cyan("[OPTIONS]").toStr();
  if (!usage.empty()) {
    str += ' ';
    str += usage;
  }
  str += '\n';
  return str;
}

void
addOptCandidates(
    std::vector<std::string_view>& candidates, const Opts& opts
) noexcept {
  for (const auto& opt : opts) {
    candidates.push_back(opt.name);
    if (!opt.shortName.empty()) {
      candidates.push_back(opt.shortName);
    }
  }
}

std::size_t
calcOptMaxShortSize(const Opts& opts) noexcept {
  std::size_t maxShortSize = 0;
  for (const auto& opt : opts) {
    if (opt.isHidden) {
      // Hidden option should not affect maxShortSize.
      continue;
    }
    maxShortSize = std::max(maxShortSize, opt.shortName.size());
  }
  return maxShortSize;
}

std::size_t
calcOptMaxOffset(const Opts& opts, const std::size_t maxShortSize) noexcept {
  std::size_t maxOffset = 0;
  for (const auto& opt : opts) {
    if (opt.isHidden) {
      // Hidden option should not affect maxOffset.
      continue;
    }
    maxOffset = std::max(maxOffset, opt.leftSize(maxShortSize));
  }
  return maxOffset;
}

std::string
formatOpts(
    const Opts& opts, const std::size_t maxShortSize,
    const std::size_t maxOffset
) noexcept {
  std::string str;
  for (const auto& opt : opts) {
    if (opt.isHidden) {
      // We don't include hidden options.
      continue;
    }
    str += opt.format(maxShortSize, maxOffset);
  }
  return str;
}

std::string
Opt::format(const std::size_t maxShortSize, std::size_t maxOffset)
    const noexcept {
  std::string option;
  if (!shortName.empty()) {
    option += Bold(Cyan(shortName)).toStr();
    option += ", ";
    if (maxShortSize > shortName.size()) {
      option += std::string(maxShortSize - shortName.size(), ' ');
    }
  } else {
    // This coloring is for the alignment with std::setw later.
    option += Bold(Cyan(std::string(maxShortSize, ' '))).toStr();
    option += "  ";  // ", "
  }
  option += Bold(Cyan(name)).toStr();
  option += ' ';
  option += Cyan(placeholder).toStr();

  if (shouldColorStdout()) {
    // Color escape sequences are not visible but affect std::setw.
    constexpr std::size_t colorEscapeSeqLen = 31;
    maxOffset += colorEscapeSeqLen;
  }
  std::string str = formatLeft(maxOffset, option);
  str += desc;
  if (!defaultVal.empty()) {
    str += fmt::format(" [default: {}]", defaultVal);
  }
  str += '\n';
  return str;
}

std::string
Arg::getLeft() const noexcept {
  if (name.empty()) {
    return "";
  }

  std::string left;
  if (required) {
    left += '<';
  } else {
    left += '[';
  }
  left += name;
  if (required) {
    left += '>';
  } else {
    left += ']';
  }
  if (variadic) {
    left += "...";
  }
  return Cyan(std::move(left)).toStr();
}
std::string
Arg::format(std::size_t maxOffset) const noexcept {
  const std::string left = getLeft();
  if (shouldColorStdout()) {
    // Color escape sequences are not visible but affect std::setw.
    constexpr std::size_t colorEscapeSeqLen = 9;
    maxOffset += colorEscapeSeqLen;
  }
  std::string str = formatLeft(maxOffset, left);
  if (!desc.empty()) {
    str += desc;
  }
  str += '\n';
  return str;
}

Subcmd&
Subcmd::addOpt(Opt opt) noexcept {
  localOpts.emplace(opt);
  return *this;
}
Subcmd&
Subcmd::setMainFn(std::function<MainFn> mainFn) noexcept {
  this->mainFn = std::move(mainFn);
  return *this;
}
Subcmd&
Subcmd::setGlobalOpts(const Opts& globalOpts) noexcept {
  this->globalOpts = globalOpts;
  return *this;
}
std::string
Subcmd::formatUsage(FILE* file) const noexcept {
  std::string str = Bold(Green("Usage: ")).toStr(file);
  str += Bold(Cyan(cmdName)).toStr(file);
  str += ' ';
  str += Bold(Cyan(name)).toStr(file);
  str += ' ';
  str += Cyan("[OPTIONS]").toStr(file);
  if (!arg.name.empty()) {
    str += ' ';
    str += Cyan(arg.getLeft()).toStr(file);
  }
  return str;
}

[[nodiscard]] AnyhowErr
Subcmd::noSuchArg(std::string_view arg) const {
  std::vector<std::string_view> candidates;
  if (globalOpts.has_value()) {
    addOptCandidates(candidates, globalOpts.value());
  }
  addOptCandidates(candidates, localOpts);

  std::string suggestion;
  if (const auto similar = findSimilarStr(arg, candidates)) {
    suggestion = fmt::format(
        "{} did you mean '{}'?\n\n", Bold(Cyan("Tip:")).toErrStr(),
        Bold(Yellow(similar.value())).toErrStr()
    );
  }
  return anyhow::anyhow(
      "unexpected argument '{}' found\n\n"
      "{}"
      "{}\n\n"
      "For more information, try '{}'",
      Bold(Yellow(arg)).toErrStr(), suggestion, formatUsage(stderr),
      Bold(Cyan("--help")).toErrStr()
  );
}

[[nodiscard]] AnyhowErr
Subcmd::missingOptArgumentFor(const std::string_view arg) noexcept {
  return anyhow::anyhow("Missing argument for `{}`", arg);
}

std::size_t
Subcmd::calcMaxShortSize() const noexcept {
  std::size_t maxShortSize = 0;
  if (globalOpts.has_value()) {
    maxShortSize =
        std::max(maxShortSize, calcOptMaxShortSize(globalOpts.value()));
  }
  maxShortSize = std::max(maxShortSize, calcOptMaxShortSize(localOpts));
  return maxShortSize;
}
std::size_t
Subcmd::calcMaxOffset(const std::size_t maxShortSize) const noexcept {
  std::size_t maxOffset = 0;
  if (globalOpts.has_value()) {
    maxOffset =
        std::max(maxOffset, calcOptMaxOffset(globalOpts.value(), maxShortSize));
  }
  maxOffset = std::max(maxOffset, calcOptMaxOffset(localOpts, maxShortSize));

  if (!arg.desc.empty()) {
    // If args does not have a description, it is not necessary to consider
    // its length.
    maxOffset = std::max(maxOffset, arg.leftSize());
  }
  return maxOffset;
}

std::string
Subcmd::formatHelp() const noexcept {
  const std::size_t maxShortSize = calcMaxShortSize();
  const std::size_t maxOffset = calcMaxOffset(maxShortSize);

  std::string str = std::string(desc);
  str += "\n\n";
  str += formatUsage(stdout);
  str += "\n\n";
  str += formatHeader("Options:");
  if (globalOpts.has_value()) {
    str += formatOpts(globalOpts.value(), maxShortSize, maxOffset);
  }
  str += formatOpts(localOpts, maxShortSize, maxOffset);

  if (!arg.name.empty()) {
    str += '\n';
    str += formatHeader("Arguments:");
    str += arg.format(maxOffset);
  }
  return str;
}

std::string
Subcmd::format(std::size_t maxOffset) const noexcept {
  std::string cmdStr = Bold(Cyan(name)).toStr();
  if (hasShort()) {
    cmdStr += ", ";
    cmdStr += Bold(Cyan(shortName)).toStr();
  } else {
    // This coloring is for the alignment with std::setw later.
    cmdStr += Bold(Cyan("   ")).toStr();
  }

  if (shouldColorStdout()) {
    // Color escape sequences are not visible but affect std::setw.
    constexpr std::size_t colorEscapeSeqLen = 22;
    maxOffset += colorEscapeSeqLen;
  }
  std::string str = formatLeft(maxOffset, cmdStr);
  str += desc;
  str += '\n';
  return str;
}

Cli&
Cli::addSubcmd(const Subcmd& subcmd) noexcept {
  // NOLINTNEXTLINE(cppcoreguidelines-pro-type-const-cast)
  const_cast<Subcmd&>(subcmd).setCmdName(name).setGlobalOpts(globalOpts);

  subcmds.insert_or_assign(subcmd.name, subcmd);
  if (subcmd.hasShort()) {
    subcmds.insert_or_assign(subcmd.shortName, subcmd);
  }
  return *this;
}
Cli&
Cli::addOpt(Opt opt) noexcept {
  if (opt.isGlobal) {
    [[maybe_unused]] const auto inserted = globalOpts.emplace(opt);
    assert(inserted.second && "global option already exists");
  } else {
    [[maybe_unused]] const auto inserted = localOpts.emplace(opt);
    assert(inserted.second && "local option already exists");
  }
  return *this;
}

bool
Cli::hasSubcmd(std::string_view subcmd) const noexcept {
  return subcmds.contains(subcmd);
}

[[nodiscard]] AnyhowErr
Cli::noSuchArg(std::string_view arg) const {
  std::vector<std::string_view> candidates;
  for (const auto& cmd : subcmds) {
    candidates.push_back(cmd.second.name);
    if (!cmd.second.shortName.empty()) {
      candidates.push_back(cmd.second.shortName);
    }
  }
  addOptCandidates(candidates, globalOpts);
  addOptCandidates(candidates, localOpts);

  std::string suggestion;
  if (const auto similar = findSimilarStr(arg, candidates)) {
    suggestion = fmt::format(
        "{} did you mean '{}'?\n\n", Bold(Cyan("Tip:")).toErrStr(),
        Bold(Yellow(similar.value())).toErrStr()
    );
  }
  return anyhow::anyhow(
      "unexpected argument '{}' found\n\n"
      "{}"
      "For a list of commands, try '{}'",
      Bold(Yellow(arg)).toErrStr(), suggestion,
      Bold(Cyan("cabin help")).toErrStr()
  );
}

[[nodiscard]] Result<void>
Cli::exec(const std::string_view subcmd, const CliArgsView args) const {
  return subcmds.at(subcmd).mainFn(args);
}

[[nodiscard]] Result<Cli::ControlFlow>
Cli::handleGlobalOpts(
    std::forward_iterator auto& itr, const std::forward_iterator auto end,
    const std::string& subcmd
) {
  const std::string_view arg = *itr;

  if (arg == "-h" || arg == "--help") {
    if (!subcmd.empty()) {
      // {{ }} is a workaround for std::span until C++26.
      return getCli().printHelp({ { subcmd } }).map([] { return Return; });
    } else {
      return getCli().printHelp({}).map([] { return Return; });
    }
  } else if (arg == "-v" || arg == "--verbose") {
    setDiagLevel(DiagLevel::Verbose);
    return Ok(Continue);
  } else if (arg == "-vv") {
    setDiagLevel(DiagLevel::VeryVerbose);
    return Ok(Continue);
  } else if (arg == "-q" || arg == "--quiet") {
    setDiagLevel(DiagLevel::Off);
    return Ok(Continue);
  } else if (arg == "--color") {
    Ensure(itr + 1 < end, "missing argument for `--color`");
    setColorMode(*++itr);
    return Ok(Continue);
  }
  return Ok(Fallthrough);
}

Result<void>
Cli::parseArgs(
    const int argc, char* argv[]  // NOLINT(*-avoid-c-arrays)
) const noexcept {
  // Drop the first argument (program name)
  return parseArgs(Try(expandOpts({ argv + 1, argv + argc })));
}

Result<void>
Cli::parseArgs(const CliArgsView args) const noexcept {
  // Parse arguments (options should appear before the subcommand, as the help
  // message shows intuitively)
  // cabin --verbose run --release help --color always --verbose
  // ^^^^^^^^^^^^^^^ ^^^^^^^^^^^^^ ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
  // [global]        [run]         [help (under run)]
  for (auto itr = args.begin(); itr != args.end(); ++itr) {
    const std::string_view arg = *itr;

    // Global options
    const auto control = Try(Cli::handleGlobalOpts(itr, args.end()));
    if (control == Cli::Return) {
      return Ok();
    } else if (control == Cli::Continue) {
      continue;
    }
    // else: Fallthrough: current argument wasn't handled

    // Local options
    else if (arg == "-V" || arg == "--version") {
      return exec("version", { itr + 1, args.end() });
    } else if (arg == "--list") {
      fmt::print("{}", formatAllSubcmds(true));
      return Ok();
    }

    // Subcommands
    else if (hasSubcmd(arg)) {
      try {
        return exec(arg, { itr + 1, args.end() });
      } catch (const std::exception& e) {
        Bail(e.what());
      }
    }

    // Unexpected argument
    else {
      return noSuchArg(arg);
    }
  }

  return printHelp({});
}

Result<std::vector<std::string>>
Cli::expandOpts(const std::span<const char* const> args) const noexcept {
  struct ShortOpts {
    std::unordered_map<std::string_view, Opt> names;
    std::size_t maxShortSize = 0;

    explicit ShortOpts(const Opts& opts) {
      for (const Opt& opt : opts) {
        if (opt.hasShort()) {
          names.emplace(opt.shortName, opt);
          maxShortSize = std::max(maxShortSize, opt.shortName.size());
        }
      }
    }
  };

  std::optional<std::reference_wrapper<const Subcmd>> curSubcmd;
  std::reference_wrapper<const Opts> curLocalOpts = localOpts;
  const ShortOpts globalShortOpts{ globalOpts };
  ShortOpts curLocalShortOpts{ localOpts };
  std::size_t curMaxShortSize =
      std::max(globalShortOpts.maxShortSize, curLocalShortOpts.maxShortSize);

  std::vector<std::string> expanded;
  for (std::size_t i = 0; i < args.size(); ++i) {
    const std::string_view arg = args[i];

    // Subcmd case, remains the same as before
    if (!curSubcmd.has_value() && !arg.starts_with("-")) {
      if (!subcmds.contains(arg)) {
        return noSuchArg(arg);
      }
      expanded.emplace_back(arg);

      curSubcmd = subcmds.at(arg);
      curLocalOpts = subcmds.at(arg).localOpts;
      curLocalShortOpts = ShortOpts{ curLocalOpts.get() };
      curMaxShortSize = std::max(
          globalShortOpts.maxShortSize, curLocalShortOpts.maxShortSize
      );
      continue;
    }

    // Long option case
    //
    // "--verbose" => ["--verbose"]
    // "--color always" => ["--color", "always"]
    // "--color=always" => ["--color", "always"]
    else if (arg.starts_with("--")) {
      std::string_view optName = arg;
      const auto eqPos = arg.find_first_of('=');
      if (eqPos != std::string_view::npos) {
        optName = arg.substr(0, eqPos);
      }

      const auto handleLongOpt = [&](const auto opt) -> Result<void> {
        if (opt->takesArg()) {
          if (eqPos != std::string_view::npos) {
            if (eqPos + 1 < arg.size()) {
              // Handle "--color=always" case.
              expanded.emplace_back(optName);
              expanded.emplace_back(arg.substr(eqPos + 1));
            } else {
              // Handle "--color=" case.
              return Subcmd::missingOptArgumentFor(optName);
            }
          } else {
            if (i + 1 < args.size()) {
              // Handle "--color always" case.  Note that the validity of the
              // value will be checked later.
              expanded.emplace_back(arg);
              expanded.emplace_back(args[++i]);
            } else {
              // Handle "--color" case.
              return Subcmd::missingOptArgumentFor(arg);
            }
          }
        } else {
          expanded.emplace_back(arg);
        }
        return Ok();
      };

      auto opt = globalOpts.find(Opt{ optName });
      if (opt != globalOpts.end()) {
        Try(handleLongOpt(opt));
        continue;
      }
      opt = curLocalOpts.get().find(Opt{ optName });
      if (opt != curLocalOpts.get().end()) {
        Try(handleLongOpt(opt));
        continue;
      }
      // Unknown option is found.
    }

    // Short option case
    //
    // "-v" => ["-v"]
    // "-j1" => ["-j", "1"]
    // "-vvvrj1" => ["-vv", "-v", "-r", "-j", "1"]
    else if (arg.starts_with("-")) {
      bool handled = false;
      std::size_t left = 1;
      for (; left < arg.size(); ++left) {
        const std::size_t rightStart =
            std::min(curMaxShortSize, arg.size() - 1 - left) + left;
        for (std::size_t right = rightStart; right >= left; --right) {
          // Start from the longest option name.
          const std::string optName =
              fmt::format("-{}", arg.substr(left, right - left + 1));
          const auto handleShortOpt = [&](const Opt& opt) -> Result<void> {
            if (opt.takesArg()) {
              if (right + 1 < arg.size()) {
                // Handle "-j1" case.
                expanded.emplace_back(optName);
                expanded.emplace_back(arg.substr(right + 1));
                left = arg.size();  // Break the left loop
              } else if (i + 1 < args.size()) {
                // Handle "-j 1" case.  Note that the validity of the value will
                // be checked later.
                expanded.emplace_back(optName);
                expanded.emplace_back(args[++i]);
                // Break the loop
              } else {
                // Handle "-j" case.
                return Subcmd::missingOptArgumentFor(optName);
              }
            } else {
              expanded.emplace_back(optName);
              left = right;
            }
            return Ok();
          };

          auto opt = globalShortOpts.names.find(optName);
          if (opt != globalShortOpts.names.end()) {
            Try(handleShortOpt(opt->second));
            handled = true;
            break;
          }
          opt = curLocalShortOpts.names.find(optName);
          if (opt != curLocalShortOpts.names.end()) {
            Try(handleShortOpt(opt->second));
            handled = true;
            break;
          }
        }
      }

      if (handled) {
        continue;
      }
    }

    // Unknown arguments are added as is.
    expanded.emplace_back(arg);
  }
  return Ok(expanded);
}

void
Cli::printSubcmdHelp(const std::string_view subcmd) const noexcept {
  fmt::print("{}", subcmds.at(subcmd).formatHelp());
}

std::size_t
Cli::calcMaxShortSize() const noexcept {
  // This is for printing the help message of the cabin command itself.  So,
  // we don't need to consider the length of the subcommands' options.

  std::size_t maxShortSize = 0;
  maxShortSize = std::max(maxShortSize, calcOptMaxShortSize(globalOpts));
  maxShortSize = std::max(maxShortSize, calcOptMaxShortSize(localOpts));
  return maxShortSize;
}

std::size_t
Cli::calcMaxOffset(const std::size_t maxShortSize) const noexcept {
  std::size_t maxOffset = 0;
  maxOffset = std::max(maxOffset, calcOptMaxOffset(globalOpts, maxShortSize));
  maxOffset = std::max(maxOffset, calcOptMaxOffset(localOpts, maxShortSize));

  for (const auto& [name, cmd] : subcmds) {
    if (cmd.isHidden) {
      // Hidden command should not affect maxOffset.
      continue;
    }

    std::size_t offset = name.size();  // "build"
    if (!cmd.shortName.empty()) {
      offset += 2;                     // ", "
      offset += cmd.shortName.size();  // "b"
    }
    maxOffset = std::max(maxOffset, offset);
  }
  return maxOffset;
}

std::string
Cli::formatAllSubcmds(const bool showHidden, std::size_t maxOffset)
    const noexcept {
  for (const auto& [name, cmd] : subcmds) {
    if (!showHidden && cmd.isHidden) {
      // Hidden command should not affect maxOffset if `showHidden` is false.
      continue;
    }

    std::size_t offset = name.size();  // "build"
    if (!cmd.shortName.empty()) {
      offset += 2;                     // ", "
      offset += cmd.shortName.size();  // "b"
    }
    maxOffset = std::max(maxOffset, offset);
  }

  std::string str;
  for (const auto& [name, cmd] : subcmds) {
    if (!showHidden && cmd.isHidden) {
      // We don't print hidden subcommands if `showHidden` is false.
      continue;
    }
    if (cmd.hasShort() && name == cmd.shortName) {
      // We don't print an abbreviation.
      continue;
    }
    str += cmd.format(maxOffset);
  }
  return str;
}

std::string
Cli::formatCmdHelp() const noexcept {
  // Print help message for cabin itself
  const std::size_t maxShortSize = calcMaxShortSize();
  const std::size_t maxOffset = calcMaxOffset(maxShortSize);

  std::string str = std::string(desc);
  str += "\n\n";
  str += formatUsage(name, "", Cyan("[COMMAND]").toStr());
  str += "\n";
  str += formatHeader("Options:");
  str += formatOpts(globalOpts, maxShortSize, maxOffset);
  str += formatOpts(localOpts, maxShortSize, maxOffset);
  str += '\n';
  str += formatHeader("Commands:");
  str += formatAllSubcmds(false, maxOffset);
  str += Subcmd{ "..." }
             .setDesc(fmt::format(
                 "See all commands with {}", Bold(Cyan("--list")).toStr()
             ))
             .format(maxOffset);
  str += '\n';
  str += fmt::format(
      "See '{} {} {}' for more information on a specific command.\n",
      Bold(Cyan(name)).toStr(), Bold(Cyan("help")).toStr(),
      Cyan("<command>").toStr()
  );
  return str;
}

[[nodiscard]] Result<void>
Cli::printHelp(const CliArgsView args) const noexcept {
  // Parse args
  for (auto itr = args.begin(); itr != args.end(); ++itr) {
    const std::string_view arg = *itr;

    const auto control = Try(handleGlobalOpts(itr, args.end(), "help"));
    if (control == Return) {
      return Ok();
    } else if (control == Continue) {
      continue;
    } else if (hasSubcmd(arg)) {
      printSubcmdHelp(arg);
      return Ok();
    } else {
      // TODO: Currently assumes that `help` does not implement any additional
      // options since we are using `noSuchArg` instead of
      // `helpCmd.noSuchArg`. But we want to consider subcommands as well for
      // suggestion.
      return noSuchArg(arg);
    }
  }

  // Print help message for cabin itself
  fmt::print("{}", formatCmdHelp());
  return Ok();
}

}  // namespace cabin

#ifdef CABIN_TEST

#  include "Rustify/Tests.hpp"

namespace cabin {

const Cli&
getCli() noexcept {
  static const Cli cli =  //
      Cli{ "test" }
          .addOpt(Opt{ "--verbose" }.setShort("-v"))
          .addOpt(Opt{ "-vv" }.setShort("-vv"))
          .addOpt(Opt{ "--jobs" }.setShort("-j").setPlaceholder("<NUM>"))
          .addSubcmd(Subcmd{ "run" }.setShort("r"))
          .addSubcmd(Subcmd{ "build" }.addOpt(
              Opt{ "--target" }.setShort("-t").setPlaceholder("<TARGET>")
          ));
  return cli;
}

}  // namespace cabin

namespace tests {

using namespace cabin;  // NOLINT(build/namespaces,google-build-using-namespace)

static void
testCliExpandOpts() {
  {
    const std::vector<const char*> args{ "-vvvj4" };
    const std::vector<std::string> expected{ "-vv", "-v", "-j", "4" };
    assertEq(getCli().expandOpts(args).unwrap(), expected);
  }
  {
    const std::vector<const char*> args{ "-j4vvv" };
    const std::vector<std::string> expected{ "-j", "4vvv" };
    assertEq(getCli().expandOpts(args).unwrap(), expected);
  }
  {
    const std::vector<const char*> args{ "-vj" };
    assertEq(
        getCli().expandOpts(args).unwrap_err()->what(),
        "Missing argument for `-j`"
    );
  }
  {
    const std::vector<const char*> args{ "-j" };
    assertEq(
        getCli().expandOpts(args).unwrap_err()->what(),
        "Missing argument for `-j`"
    );
  }
  {
    const std::vector<const char*> args{ "r", "-j" };
    // 1. "-j" is not in global/run options, but it's not an expandOpts()'s
    // responsibility to check it.
    // 2. "-j" sounds to take an argument, but not taking an argument is okay.
    const std::vector<std::string> expected{ "r", "-j" };
    assertEq(getCli().expandOpts(args).unwrap(), expected);
  }
  {
    // Passing "run" to the program?
    const std::vector<const char*> args{ "run", "run" };
    const std::vector<std::string> expected{ "run", "run" };
    assertEq(getCli().expandOpts(args).unwrap(), expected);
  }
  {
    // "subcmd" is not a subcommand, but possibly passing it to the program.
    const std::vector<const char*> args{ "run", "subcmd" };
    const std::vector<std::string> expected{ "run", "subcmd" };
    assertEq(getCli().expandOpts(args).unwrap(), expected);
  }
  {
    const std::vector<const char*> args{ "build", "-t" };
    assertEq(
        getCli().expandOpts(args).unwrap_err()->what(),
        "Missing argument for `-t`"
    );
  }
  {
    const std::vector<const char*> args{ "build", "--target=this" };
    const std::vector<std::string> expected{ "build", "--target", "this" };
    assertEq(getCli().expandOpts(args).unwrap(), expected);
  }
  {
    const std::vector<const char*> args{ "build", "--target=", "this" };
    const std::vector<std::string> expected{ "build", "--target=", "this" };
    assertEq(
        getCli().expandOpts(args).unwrap_err()->what(),
        "Missing argument for `--target`"
    );
  }
  {
    const std::vector<const char*> args{ "build", "--target", "this" };
    const std::vector<std::string> expected{ "build", "--target", "this" };
    assertEq(getCli().expandOpts(args).unwrap(), expected);
  }
  {
    const std::vector<const char*> args{ "-vv", "build", "--target", "this" };
    const std::vector<std::string> expected{ "-vv", "build", "--target",
                                             "this" };
    assertEq(getCli().expandOpts(args).unwrap(), expected);
  }
  {
    // "subcmd" is not a subcommand, but possibly "build"'s argument.
    const std::vector<const char*> args{ "build", "subcmd" };
    const std::vector<std::string> expected{ "build", "subcmd" };
    assertEq(getCli().expandOpts(args).unwrap(), expected);
  }
  {
    // "subcmd" is not a subcommand.
    const std::vector<const char*> args{ "subcmd", "build" };
    assertEq(
        getCli().expandOpts(args).unwrap_err()->what(),
        R"(unexpected argument 'subcmd' found

For a list of commands, try 'cabin help')"
    );
  }
  {
    // "built" is not a subcommand, but typo of "build"?
    const std::vector<const char*> args{ "built" };
    assertEq(
        getCli().expandOpts(args).unwrap_err()->what(),
        R"(unexpected argument 'built' found

Tip: did you mean 'build'?

For a list of commands, try 'cabin help')"
    );
  }

  pass();
}

}  // namespace tests

int
main() {
  cabin::setColorMode("never");

  tests::testCliExpandOpts();
}

#endif
