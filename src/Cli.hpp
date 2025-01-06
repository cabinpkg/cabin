#pragma once

#include "Logger.hpp"
#include "Rustify/Aliases.hpp"
#include "TermColor.hpp"

#include <cstdlib>
#include <functional>
#include <iterator>
#include <optional>
#include <span>
#include <string>
#include <string_view>
#include <unordered_map>
#include <vector>

class Opt;
class Arg;
class Subcmd;
class Cli;

// Defined in main.cc
const Cli& getCli() noexcept;

template <typename Derived>
class CliBase {
protected:
  // NOLINTBEGIN(cppcoreguidelines-non-private-member-variables-in-classes,misc-non-private-member-variables-in-classes)
  std::string_view name;
  std::string_view desc;
  // NOLINTEND(cppcoreguidelines-non-private-member-variables-in-classes,misc-non-private-member-variables-in-classes)

public:
  constexpr CliBase() noexcept = default;
  constexpr ~CliBase() noexcept = default;
  constexpr CliBase(const CliBase&) noexcept = default;
  constexpr CliBase(CliBase&&) noexcept = default;
  constexpr CliBase& operator=(const CliBase&) noexcept = default;
  constexpr CliBase& operator=(CliBase&&) noexcept = default;

  constexpr explicit CliBase(const std::string_view name) noexcept
      : name(name) {}
  constexpr Derived& setDesc(const std::string_view desc) noexcept {
    this->desc = desc;
    return static_cast<Derived&>(*this);
  }
};

template <typename Derived>
class ShortAndHidden {
protected:
  // NOLINTBEGIN(cppcoreguidelines-non-private-member-variables-in-classes,misc-non-private-member-variables-in-classes)
  std::string_view shortName;
  bool isHidden = false;
  // NOLINTEND(cppcoreguidelines-non-private-member-variables-in-classes,misc-non-private-member-variables-in-classes)

public:
  constexpr Derived& setShort(const std::string_view shortName) noexcept {
    this->shortName = shortName;
    return static_cast<Derived&>(*this);
  }
  constexpr Derived& setHidden(const bool isHidden) noexcept {
    this->isHidden = isHidden;
    return static_cast<Derived&>(*this);
  }
};

class Opt : public CliBase<Opt>, public ShortAndHidden<Opt> {
  friend class Subcmd;
  friend class Cli;

  std::string_view placeholder;
  std::string_view defaultVal;
  bool isGlobal = false;

public:
  using CliBase::CliBase;

  friend void addOptCandidates(
      std::vector<std::string_view>& candidates, const std::vector<Opt>& opts
  ) noexcept;
  friend std::size_t calcOptMaxShortSize(const std::vector<Opt>& opts) noexcept;
  friend std::size_t calcOptMaxOffset(
      const std::vector<Opt>& opts, std::size_t maxShortSize
  ) noexcept;
  friend void printOpts(
      const std::vector<Opt>& opts, std::size_t maxShortSize,
      std::size_t maxOffset
  ) noexcept;

  constexpr Opt& setPlaceholder(const std::string_view placeholder) noexcept {
    this->placeholder = placeholder;
    return *this;
  }
  constexpr Opt& setDefault(const std::string_view defaultVal) noexcept {
    this->defaultVal = defaultVal;
    return *this;
  }
  constexpr Opt& setGlobal(const bool isGlobal) noexcept {
    this->isGlobal = isGlobal;
    return *this;
  }

private:
  /// Size of `-c, --color <WHEN>` without color.
  constexpr std::size_t leftSize(std::size_t maxShortSize) const noexcept {
    // shrt.size() = ?
    // `, `.size() = 2
    // lng.size() = ?
    // ` `.size() = 1
    // placeholder.size() = ?
    return 3 + maxShortSize + name.size() + placeholder.size();
  }

  void print(std::size_t maxShortSize, std::size_t maxOffset) const noexcept;
};

class Arg : public CliBase<Arg> {
  friend class Subcmd;

  bool required = true;
  bool variadic = false;

public:
  using CliBase::CliBase;

  constexpr Arg& setRequired(const bool required) noexcept {
    this->required = required;
    return *this;
  }
  constexpr Arg& setVariadic(const bool variadic) noexcept {
    this->variadic = variadic;
    return *this;
  }

private:
  /// Size of left side of the help message.
  constexpr std::size_t leftSize() const noexcept {
    return name.size();
  }

  std::string getLeft() const noexcept;
  void print(std::size_t maxOffset) const noexcept;
};

class Subcmd : public CliBase<Subcmd>, public ShortAndHidden<Subcmd> {
  friend class Cli;

  std::string_view cmdName;
  std::optional<std::vector<Opt>> globalOpts = std::nullopt;
  std::vector<Opt> localOpts;
  Arg arg;
  std::function<int(std::span<const std::string_view>)> mainFn;

public:
  using CliBase::CliBase;

  constexpr Subcmd& setArg(Arg arg) noexcept {
    this->arg = arg;
    return *this;
  }

  Subcmd& addOpt(Opt opt) noexcept;
  Subcmd& setMainFn(std::function<int(std::span<const std::string_view>)> mainFn
  ) noexcept;
  [[nodiscard]] int noSuchArg(std::string_view arg) const;
  [[nodiscard]] static int missingArgumentForOpt(std::string_view arg);

private:
  constexpr bool hasShort() const noexcept {
    return !shortName.empty();
  }
  constexpr Subcmd& setCmdName(std::string_view cmdName) noexcept {
    this->cmdName = cmdName;
    return *this;
  }

  Subcmd& setGlobalOpts(const std::vector<Opt>& globalOpts) noexcept;
  std::string getUsage() const noexcept;
  void printHelp() const noexcept;
  void print(std::size_t maxOffset) const noexcept;

  std::size_t calcMaxShortSize() const noexcept;
  /// Calculate the maximum length of the left side of the helps to align the
  /// descriptions with 2 spaces.
  std::size_t calcMaxOffset(std::size_t maxShortSize) const noexcept;
};

class Cli : public CliBase<Cli> {
  std::unordered_map<std::string_view, Subcmd> subcmds;
  std::vector<Opt> globalOpts;
  std::vector<Opt> localOpts;

public:
  using CliBase::CliBase;

  Cli& addSubcmd(const Subcmd& subcmd) noexcept;
  Cli& addOpt(Opt opt) noexcept;
  bool hasSubcmd(std::string_view subcmd) const noexcept;

  [[nodiscard]] int noSuchArg(std::string_view arg) const;
  [[nodiscard]] int
  exec(std::string_view subcmd, std::span<const std::string_view> args) const;
  void printSubcmdHelp(std::string_view subcmd) const noexcept;
  [[nodiscard]] int printHelp(std::span<const std::string_view> args
  ) const noexcept;
  std::size_t calcMaxOffset(std::size_t maxShortSize) const noexcept;
  void
  printAllSubcmds(bool showHidden, std::size_t maxOffset = 0) const noexcept;

  static constexpr int CONTINUE = -1;

  // Returns the exit code if the global option was handled, otherwise
  // std::nullopt. Returns CONTINUE if the caller should not propagate the exit
  // code.
  // TODO: -1 is not a good idea.
  // TODO: result-like types make more sense.
  [[nodiscard]] static std::optional<int> handleGlobalOpts(
      std::forward_iterator auto& itr, const std::forward_iterator auto end,
      std::string_view subcmd = ""
  ) {
    if (*itr == "-h"sv || *itr == "--help"sv) {
      if (!subcmd.empty()) {
        // {{ }} is a workaround for std::span until C++26.
        return getCli().printHelp({ { subcmd } });
      } else {
        return getCli().printHelp({});
      }
    } else if (*itr == "-v"sv || *itr == "--verbose"sv) {
      logger::setLevel(logger::Level::Debug);
      return CONTINUE;
    } else if (*itr == "-vv"sv) {
      logger::setLevel(logger::Level::Trace);
      return CONTINUE;
    } else if (*itr == "-q"sv || *itr == "--quiet"sv) {
      logger::setLevel(logger::Level::Off);
      return CONTINUE;
    } else if (*itr == "--color"sv) {
      if (itr + 1 < end) {
        setColorMode(*++itr);
        return CONTINUE;
      } else {
        logger::error("missing argument for `--color`");
        return EXIT_FAILURE;
      }
    }
    return std::nullopt;
  }

private:
  std::vector<std::string_view> transformOptions(
      std::string_view subcmd, std::span<const std::string_view> args
  ) const;

  std::size_t calcMaxShortSize() const noexcept;

  /// Print help message for cabin itself.
  void printCmdHelp() const noexcept;
};
