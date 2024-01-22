#include "Global.hpp"

#include "../Algos.hpp"
#include "../Rustify.hpp"
#include "../TermColor.hpp"

#include <algorithm>
#include <cstdlib>
#include <iomanip>
#include <iostream>

bool
commandExists(const StringRef cmd) noexcept {
  String checkCmd = "command -v ";
  checkCmd += cmd;
  checkCmd += " >/dev/null 2>&1";
  return runCmd(checkCmd) == EXIT_SUCCESS;
}

void
printHeader(const StringRef header) noexcept {
  std::cout << bold(green(header)) << '\n';
}

void
printUsage(const StringRef cmd, const StringRef usage) noexcept {
  std::cout << bold(green("Usage: ")) << bold(cyan("poac "));
  if (!cmd.empty()) {
    std::cout << bold(cyan(cmd)) << ' ';
  }
  std::cout << cyan("[OPTIONS]");
  if (!usage.empty()) {
    std::cout << " " << cyan(usage);
  }
  std::cout << '\n';
}

void
printCommand(
    const StringRef name, const StringRef desc, const bool hasShort
) noexcept {
  String cmd = bold(cyan(name));
  if (hasShort) {
    cmd += ", ";
    cmd += bold(cyan(StringRef(name.data(), 1)));
  } else {
    // This coloring is for the alignment with std::setw later.
    cmd += bold(cyan("   "));
  }

  std::cout << "  " << std::left;
  if (shouldColor()) {
    std::cout << std::setw(44);
  } else {
    std::cout << std::setw(10);
  }
  std::cout << cmd << desc << '\n';
}

void
printGlobalOpts(const usize maxOptLen) noexcept {
  for (const auto& opt : GLOBAL_OPTS) {
    opt.print(maxOptLen);
  }
}

String
Opt::toString(const bool forceColor) const noexcept {
  String str;
  if (!shrt.empty()) {
    str += bold(cyan(shrt, forceColor), forceColor);
    str += ", ";
  } else {
    // This coloring is for the alignment with std::setw later.
    str += bold(cyan("    ", forceColor), forceColor);
  }
  str += bold(cyan(lng, forceColor), forceColor);
  str += ' ';
  str += cyan(placeholder, forceColor);
  return str;
}

void
Opt::print(usize maxOptLen) const noexcept {
  // TODO: Redundant toString call here and in Subcmd::finalize.
  const String option = toString();
  std::cout << "  " << std::left;
  if (shouldColor()) {
    std::cout << std::setw(static_cast<int>(maxOptLen) + 2);
  } else {
    std::cout << std::setw(static_cast<int>(maxOptLen) - 41);
  }
  std::cout << option << desc;
  if (!defaultVal.empty()) {
    std::cout << " [default: " << defaultVal << ']';
  }
  std::cout << '\n';
}

Subcmd&
Subcmd::setDesc(StringRef desc) noexcept {
  this->desc = desc;
  return *this;
}
Subcmd&
Subcmd::addOpt(const Opt& opt) noexcept {
  opts.emplace_back(opt);
  return *this;
}
Subcmd&
Subcmd::setArg(const Arg& arg) noexcept {
  this->arg = arg;
  return *this;
}
Subcmd&
Subcmd::finalize() noexcept {
  // We do forceColor here to get consistent maxOptLen regardless of the
  // value of ColorState.  This is because this function can be called
  // when we initialize Subcmd objects, such as BUILD_CMD, meaning that
  // this function will be called before ColorState is initialized through
  // the main function.  But with POAC_TERM_COLOR, the ColorState will be
  // set to an arbitrary value, so this can cause inconsistent maxOptLen.
  // TODO: Can't we streamline this?
  for (const auto& opt : GLOBAL_OPTS) {
    maxOptLen = std::max(maxOptLen, opt.toString(true).size());
  }
  for (const auto& opt : opts) {
    maxOptLen = std::max(maxOptLen, opt.toString(true).size());
  }
  return *this;
}

[[nodiscard]] int
Subcmd::noSuchArg(StringRef arg) const {
  Vec<StringRef> candidates;
  for (const auto& opt : GLOBAL_OPTS) {
    candidates.push_back(opt.lng);
    if (!opt.shrt.empty()) {
      candidates.push_back(opt.shrt);
    }
  }
  for (const auto& opt : opts) {
    candidates.push_back(opt.lng);
    if (!opt.shrt.empty()) {
      candidates.push_back(opt.shrt);
    }
  }

  String suggestion;
  if (const auto similar = findSimilarStr(arg, candidates)) {
    suggestion = "       Did you mean `" + String(similar.value()) + "`?\n\n";
  }
  Logger::error(
      "no such argument: `", arg, "`\n\n", suggestion, "       Run `poac help ",
      name, "` for a list of arguments"
  );
  return EXIT_FAILURE;
}

void
Subcmd::printHelp() const noexcept {
  std::cout << desc << '\n';
  std::cout << '\n';

  printUsage(name, arg.name);
  std::cout << '\n';

  printHeader("Options:");
  printGlobalOpts(maxOptLen);
  for (const auto& opt : opts) {
    opt.print(maxOptLen);
  }

  if (!arg.name.empty()) {
    std::cout << '\n';
    printHeader("Arguments:");
    std::cout << "  " << arg.name;
    if (!arg.desc.empty()) {
      std::cout << '\t' << arg.desc;
    }
    std::cout << '\n';
  }
}
