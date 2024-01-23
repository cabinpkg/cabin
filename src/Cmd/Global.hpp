// Global options and helper functions for commands.

#pragma once

#include "../Algos.hpp"
#include "../Logger.hpp"
#include "../Rustify.hpp"
#include "../TermColor.hpp"
#include "Help.hpp"

#include <cstdlib>
#include <ostream>
#include <tuple>

#define HANDLE_GLOBAL_OPTS(HELP_ARGS)                  \
  if (arg == "-h" || arg == "--help") {                \
    return helpMain(HELP_ARGS);                        \
  } else if (arg == "-v" || arg == "--verbose") {      \
    Logger::setLevel(LogLevel::debug);                 \
  } else if (arg == "-q" || arg == "--quiet") {        \
    Logger::setLevel(LogLevel::off);                   \
  } else if (arg == "--color") {                       \
    if (i + 1 < args.size()) {                         \
      setColorMode(args[++i]);                         \
    } else {                                           \
      Logger::error("missing argument for `--color`"); \
      return EXIT_FAILURE;                             \
    }                                                  \
  }

struct Opt {
  StringRef lng;
  StringRef shrt;
  StringRef desc;
  StringRef placeholder;
  StringRef defaultVal;

  constexpr Opt() noexcept = default;
  constexpr ~Opt() noexcept = default;
  constexpr Opt(const Opt&) noexcept = default;
  constexpr Opt(Opt&&) noexcept = default;
  constexpr Opt& operator=(const Opt&) noexcept = default;
  constexpr Opt& operator=(Opt&&) noexcept = default;

  explicit constexpr Opt(StringRef lng, StringRef shrt = "") noexcept
      : lng(lng), shrt(shrt) {}

  inline constexpr Opt setDesc(StringRef desc) noexcept {
    this->desc = desc;
    return *this;
  }
  inline constexpr Opt setPlaceholder(StringRef placeholder) noexcept {
    this->placeholder = placeholder;
    return *this;
  }
  inline constexpr Opt setDefault(StringRef defaultVal) noexcept {
    this->defaultVal = defaultVal;
    return *this;
  }

  /// Size of `-c, --color <WHEN>` without color.  shrt size should always
  /// be 2.
  usize leftSize() const noexcept;
  void print(usize maxOffset) const noexcept;
};

inline constinit const Arr<Opt, 4> GLOBAL_OPTS{
  Opt{ "--verbose", "-v" }.setDesc("Use verbose output"),
  Opt{ "--quiet", "-q" }.setDesc("Do not print poac log messages"),
  Opt{ "--color" }
      .setDesc("Coloring: auto, always, never")
      .setPlaceholder("<WHEN>"),
  Opt{ "--help", "-h" }.setDesc("Print help"),
};

struct Arg {
  StringRef name;
  StringRef desc;

  constexpr Arg() noexcept = default;
  constexpr ~Arg() noexcept = default;
  constexpr Arg(const Arg&) noexcept = default;
  constexpr Arg(Arg&&) noexcept = default;
  constexpr Arg& operator=(const Arg&) noexcept = default;
  constexpr Arg& operator=(Arg&&) noexcept = default;

  explicit constexpr Arg(StringRef name) noexcept : name(name) {}

  inline constexpr Arg setDesc(StringRef desc) noexcept {
    this->desc = desc;
    return *this;
  }

  /// Size of left side of the help message.
  usize leftSize() const noexcept;
};

class Subcmd {
  StringRef name;
  StringRef shortName;
  StringRef desc;
  Vec<Opt> opts;
  Arg arg;

public:
  Subcmd() noexcept = delete;
  ~Subcmd() noexcept = default;
  Subcmd(const Subcmd&) noexcept = default;
  Subcmd(Subcmd&&) noexcept = default;
  Subcmd& operator=(const Subcmd&) noexcept = default;
  Subcmd& operator=(Subcmd&&) noexcept = default;

  explicit Subcmd(StringRef name) noexcept : name(name) {}

  Subcmd& setDesc(StringRef desc) noexcept;
  StringRef getDesc() const noexcept;
  Subcmd& setShort(StringRef shortName) noexcept;
  bool hasShort() const noexcept;
  Subcmd& addOpt(const Opt& opt) noexcept;
  Subcmd& setArg(const Arg& arg) noexcept;

  [[nodiscard]] int noSuchArg(StringRef arg) const;
  void printHelp() const noexcept;

private:
  /// Calculate the maximum length of the left side of the helps to align the
  /// descriptions with 2 spaces.
  usize calcMaxOffset() const noexcept;
};

bool commandExists(StringRef cmd) noexcept;
void printHeader(StringRef header) noexcept;
void printUsage(StringRef cmd, StringRef usage) noexcept;
void printCommand(
    StringRef name, StringRef desc, bool hasShort, usize maxOffset
) noexcept;
void printGlobalOpts(usize maxOffset) noexcept;
