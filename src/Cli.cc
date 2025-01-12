#include "Cli.hpp"

#include "Algos.hpp"
#include "Rustify/Result.hpp"
#include "TermColor.hpp"

#include <algorithm>
#include <cstdlib>
#include <fmt/core.h>
#include <functional>
#include <string>
#include <string_view>
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
    std::vector<std::string_view>& candidates, const std::vector<Opt>& opts
) noexcept {
  for (const auto& opt : opts) {
    candidates.push_back(opt.name);
    if (!opt.shortName.empty()) {
      candidates.push_back(opt.shortName);
    }
  }
}

std::size_t
calcOptMaxShortSize(const std::vector<Opt>& opts) noexcept {
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
calcOptMaxOffset(
    const std::vector<Opt>& opts, const std::size_t maxShortSize
) noexcept {
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
    const std::vector<Opt>& opts, const std::size_t maxShortSize,
    const std::size_t maxOffset
) noexcept {
  std::string str;
  for (const auto& opt : opts) {
    if (opt.isHidden) {
      // We don't format hidden options.
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
  localOpts.emplace_back(opt);
  return *this;
}
Subcmd&
Subcmd::setMainFn(std::function<MainFn> mainFn) noexcept {
  this->mainFn = std::move(mainFn);
  return *this;
}
Subcmd&
Subcmd::setGlobalOpts(const std::vector<Opt>& globalOpts) noexcept {
  this->globalOpts = globalOpts;
  return *this;
}
std::string
Subcmd::formatUsage() const noexcept {
  std::string str = Bold(Green("Usage: ")).toStr();
  str += Bold(Cyan(cmdName)).toStr();
  str += ' ';
  str += Bold(Cyan(name)).toStr();
  str += ' ';
  str += Cyan("[OPTIONS]").toStr();
  if (!arg.name.empty()) {
    str += ' ';
    str += Cyan(arg.getLeft()).toStr();
  }
  return str;
}

[[nodiscard]] Result<void>
Subcmd::noSuchArg(std::string_view arg) const {
  std::vector<std::string_view> candidates;
  if (globalOpts.has_value()) {
    addOptCandidates(candidates, globalOpts.value());
  }
  addOptCandidates(candidates, localOpts);

  std::string suggestion;
  if (const auto similar = findSimilarStr(arg, candidates)) {
    suggestion = fmt::format(
        "{} did you mean '{}'?\n\n", Bold(Cyan("Tip:")).toStr(),
        Bold(Yellow(similar.value())).toStr()
    );
  }
  Bail(
      "unexpected argument '{}' found\n\n"
      "{}"
      "{}\n\n"
      "For more information, try '{}'",
      Bold(Yellow(arg)).toStr(), suggestion, formatUsage(),
      Bold(Cyan("--help")).toStr()
  );
}

[[nodiscard]] Result<void>
Subcmd::missingOptArgument(const std::string_view arg) noexcept {
  Bail("Missing argument for `{}`", arg);
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
  str += formatUsage();
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
    globalOpts.emplace_back(opt);
  } else {
    localOpts.emplace_back(opt);
  }
  return *this;
}

bool
Cli::hasSubcmd(std::string_view subcmd) const noexcept {
  return subcmds.contains(subcmd);
}

[[nodiscard]] Result<void>
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
  Bail(
      "unexpected argument '{}' found\n\n"
      "{}"
      "For a list of commands, try '{}'",
      Bold(Yellow(arg)).toErrStr(), suggestion,
      Bold(Cyan("cabin help")).toErrStr()
  );
}

[[nodiscard]] Result<void>
Cli::exec(
    const std::string_view subcmd, const std::span<const std::string_view> args
) const {
  return subcmds.at(subcmd).mainFn(transformOptions(subcmd, args));
}

std::vector<std::string_view>
Cli::transformOptions(
    std::string_view subcmd, std::span<const std::string_view> args
) const {
  const Subcmd& cmd = subcmds.at(subcmd);
  std::vector<std::string_view> transformed;
  transformed.reserve(args.size());
  for (std::size_t argIdx = 0; argIdx < args.size(); ++argIdx) {
    const std::string_view arg = args[argIdx];

    if (arg.starts_with("--")) {
      if (auto pos = arg.find_first_of('='); pos != std::string_view::npos) {
        transformed.push_back(arg.substr(0, pos));
        transformed.push_back(arg.substr(pos + 1));
        continue;
      }
    } else if (arg.starts_with("-")) {
      std::string_view multioption = arg.substr(1);
      bool handled = false;
      for (std::size_t i = 0; i < multioption.size(); ++i) {
        const auto handle = [&](const std::span<const Opt> opts) {
          for (const Opt& opt : opts) {
            if (opt.shortName.empty()) {
              continue;
            }
            if (opt.shortName.substr(1) != multioption.substr(i, 1)) {
              continue;
            }
            transformed.push_back(opt.shortName);
            // Placeholder is not empty means that this option takes a value.
            if (!opt.placeholder.empty()) {
              if (i + 1 < multioption.size()) {
                // Handle concatenated value (like -j1)
                transformed.push_back(multioption.substr(i + 1));
              } else if (argIdx + 1 < args.size()
                         && !args[argIdx + 1].starts_with("-")) {
                // Handle space-separated value (like -j 1)
                transformed.push_back(args[++argIdx]
                );  // Consume the next argument as the option's value
              }
            }
            handled = true;
          }
        };
        if (cmd.globalOpts) {
          handle(*cmd.globalOpts);
        }
        handle(cmd.localOpts);
      }
      if (handled) {
        continue;
      }
    }

    transformed.push_back(arg);
  }
  return transformed;
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
Cli::printHelp(const std::span<const std::string_view> args) const noexcept {
  // Parse args
  for (auto itr = args.begin(); itr != args.end(); ++itr) {
    const auto control = Try(handleGlobalOpts(itr, args.end(), "help"));
    if (control == Return) {
      return Ok();
    } else if (control == Continue) {
      continue;
    } else if (hasSubcmd(*itr)) {
      printSubcmdHelp(*itr);
      return Ok();
    } else {
      // TODO: Currently assumes that `help` does not implement any additional
      // options since we are using `noSuchArg` instead of
      // `helpCmd.noSuchArg`. But we want to consider subcommands as well for
      // suggestion.
      return noSuchArg(*itr);
    }
  }

  // Print help message for cabin itself
  fmt::print("{}", formatCmdHelp());
  return Ok();
}

}  // namespace cabin
