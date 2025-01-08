#include "Cli.hpp"

#include "Algos.hpp"
#include "Logger.hpp"
#include "TermColor.hpp"

#include <algorithm>
#include <cstdlib>
#include <fmt/core.h>
#include <functional>
#include <iomanip>
#include <iostream>
#include <string>
#include <string_view>
#include <utility>
#include <vector>

namespace cabin {

static constinit const std::string_view PADDING = "  ";

static void
setOffset(const std::size_t offset) noexcept {
  std::cout << PADDING << std::left
            << std::setw(static_cast<int>(offset + PADDING.size()));
}

static void
printHeader(const std::string_view header) noexcept {
  std::cout << bold(green(header)) << '\n';
}

static void
printUsage(
    const std::string_view name, const std::string_view cmd,
    const std::string_view usage
) noexcept {
  std::cout << bold(green("Usage: ")) << bold(cyan(name)) << ' ';
  if (!cmd.empty()) {
    std::cout << bold(cyan(cmd)) << ' ';
  }
  std::cout << cyan("[OPTIONS]");
  if (!usage.empty()) {
    std::cout << " " << usage;
  }
  std::cout << '\n';
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

void
printOpts(
    const std::vector<Opt>& opts, const std::size_t maxShortSize,
    const std::size_t maxOffset
) noexcept {
  for (const auto& opt : opts) {
    if (opt.isHidden) {
      // We don't print hidden options.
      continue;
    }
    opt.print(maxShortSize, maxOffset);
  }
}

void
Opt::print(const std::size_t maxShortSize, std::size_t maxOffset)
    const noexcept {
  std::string option;
  if (!shortName.empty()) {
    option += bold(cyan(shortName));
    option += ", ";
    if (maxShortSize > shortName.size()) {
      option += std::string(maxShortSize - shortName.size(), ' ');
    }
  } else {
    // This coloring is for the alignment with std::setw later.
    option += bold(cyan(std::string(maxShortSize, ' ')));
    option += "  ";  // ", "
  }
  option += bold(cyan(name));
  option += ' ';
  option += cyan(placeholder);

  if (shouldColor()) {
    // Color escape sequences are not visible but affect std::setw.
    constexpr std::size_t colorEscapeSeqLen = 31;
    maxOffset += colorEscapeSeqLen;
  }
  setOffset(maxOffset);
  std::cout << option << desc;
  if (!defaultVal.empty()) {
    std::cout << " [default: " << defaultVal << ']';
  }
  std::cout << '\n';
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
  return cyan(left);
}
void
Arg::print(std::size_t maxOffset) const noexcept {
  const std::string left = getLeft();
  if (shouldColor()) {
    // Color escape sequences are not visible but affect std::setw.
    constexpr std::size_t colorEscapeSeqLen = 9;
    maxOffset += colorEscapeSeqLen;
  }
  setOffset(maxOffset);
  std::cout << left;
  if (!desc.empty()) {
    std::cout << desc;
  }
  std::cout << '\n';
}

Subcmd&
Subcmd::addOpt(Opt opt) noexcept {
  localOpts.emplace_back(opt);
  return *this;
}
Subcmd&
Subcmd::setMainFn(std::function<int(std::span<const std::string_view>)> mainFn
) noexcept {
  this->mainFn = std::move(mainFn);
  return *this;
}
Subcmd&
Subcmd::setGlobalOpts(const std::vector<Opt>& globalOpts) noexcept {
  this->globalOpts = globalOpts;
  return *this;
}
std::string
Subcmd::getUsage() const noexcept {
  std::string str = bold(green("Usage: "));
  str += bold(cyan(cmdName));
  str += ' ';
  str += bold(cyan(name));
  str += ' ';
  str += cyan("[OPTIONS]");
  if (!arg.name.empty()) {
    str += ' ';
    str += cyan(arg.getLeft());
  }
  return str;
}

[[nodiscard]] int
Subcmd::noSuchArg(std::string_view arg) const {
  std::vector<std::string_view> candidates;
  if (globalOpts.has_value()) {
    addOptCandidates(candidates, globalOpts.value());
  }
  addOptCandidates(candidates, localOpts);

  std::string suggestion;
  if (const auto similar = findSimilarStr(arg, candidates)) {
    suggestion = bold(cyan("  Tip:")) + " did you mean '"
                 + bold(yellow(similar.value())) + "'?\n\n";
  }
  logger::error(
      "unexpected argument '{}' found\n\n"
      "{}"
      "{}\n\n"
      "For more information, try '{}'",
      bold(yellow(arg)), suggestion, getUsage(), bold(cyan("--help"))
  );
  return EXIT_FAILURE;
}

[[nodiscard]] int
Subcmd::missingArgumentForOpt(const std::string_view arg) {
  logger::error("Missing argument for `{}`", arg);
  return EXIT_FAILURE;
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

void
Subcmd::printHelp() const noexcept {
  const std::size_t maxShortSize = calcMaxShortSize();
  const std::size_t maxOffset = calcMaxOffset(maxShortSize);

  std::cout << desc << '\n';
  std::cout << '\n';
  std::cout << getUsage() << "\n\n";

  printHeader("Options:");
  if (globalOpts.has_value()) {
    printOpts(globalOpts.value(), maxShortSize, maxOffset);
  }
  printOpts(localOpts, maxShortSize, maxOffset);

  if (!arg.name.empty()) {
    std::cout << '\n';
    printHeader("Arguments:");
    arg.print(maxOffset);
  }
}

void
Subcmd::print(std::size_t maxOffset) const noexcept {
  std::string cmdStr = bold(cyan(name));
  if (hasShort()) {
    cmdStr += ", ";
    cmdStr += bold(cyan(shortName));
  } else {
    // This coloring is for the alignment with std::setw later.
    cmdStr += bold(cyan("   "));
  }

  if (shouldColor()) {
    // Color escape sequences are not visible but affect std::setw.
    constexpr std::size_t colorEscapeSeqLen = 22;
    maxOffset += colorEscapeSeqLen;
  }
  setOffset(maxOffset);
  std::cout << cmdStr << desc << '\n';
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

[[nodiscard]] int
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
    suggestion = bold(cyan("  Tip:")) + " did you mean '"
                 + bold(yellow(similar.value())) + "'?\n\n";
  }
  logger::error(
      "unexpected argument '{}' found\n\n"
      "{}"
      "For a list of commands, try '{}'",
      bold(yellow(arg)), suggestion, bold(cyan("cabin help"))
  );
  return EXIT_FAILURE;
}

[[nodiscard]] int
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
  subcmds.at(subcmd).printHelp();
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

void
Cli::printAllSubcmds(const bool showHidden, std::size_t maxOffset)
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

  for (const auto& [name, cmd] : subcmds) {
    if (!showHidden && cmd.isHidden) {
      // We don't print hidden subcommands if `showHidden` is false.
      continue;
    }
    if (cmd.hasShort() && name == cmd.shortName) {
      // We don't print an abbreviation.
      continue;
    }
    cmd.print(maxOffset);
  }
}

void
Cli::printCmdHelp() const noexcept {
  // Print help message for cabin itself
  const std::size_t maxShortSize = calcMaxShortSize();
  const std::size_t maxOffset = calcMaxOffset(maxShortSize);

  std::cout << desc << '\n';
  std::cout << '\n';
  printUsage(name, "", cyan("[COMMAND]"));
  std::cout << '\n';

  printHeader("Options:");
  printOpts(globalOpts, maxShortSize, maxOffset);
  printOpts(localOpts, maxShortSize, maxOffset);
  std::cout << '\n';

  printHeader("Commands:");
  printAllSubcmds(false, maxOffset);

  const std::string dummyDesc = "See all commands with " + bold(cyan("--list"));
  Subcmd{ "..." }.setDesc(dummyDesc).print(maxOffset);

  std::cout
      << '\n'
      << fmt::format(
             "See '{} {} {}' for more information on a specific command.\n",
             bold(cyan(name)), bold(cyan("help")), cyan("<command>")
         );
}

[[nodiscard]] int
Cli::printHelp(const std::span<const std::string_view> args) const noexcept {
  // Parse args
  for (auto itr = args.begin(); itr != args.end(); ++itr) {
    if (const auto res = handleGlobalOpts(itr, args.end(), "help")) {
      if (res.value() == Cli::CONTINUE) {
        continue;
      } else {
        return res.value();
      }
    } else if (hasSubcmd(*itr)) {
      printSubcmdHelp(*itr);
      return EXIT_SUCCESS;
    } else {
      // TODO: Currently assumes that `help` does not implement any additional
      // options since we are using `noSuchArg` instead of
      // `helpCmd.noSuchArg`. But we want to consider subcommands as well for
      // suggestion.
      return noSuchArg(*itr);
    }
  }

  // Print help message for cabin itself
  printCmdHelp();
  return EXIT_SUCCESS;
}

}  // namespace cabin
