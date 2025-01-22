#pragma once

#include "Logger.hpp"
#include "Rustify/Result.hpp"
#include "TermColor.hpp"

#include <cstdlib>
#include <functional>
#include <iostream>
#include <iterator>
#include <optional>
#include <span>
#include <string>
#include <string_view>
#include <unordered_map>
#include <vector>

namespace cabin {

class Opt;
class Arg;
class Subcmd;
class Cli;

// Defined in main.cc
const Cli& getCli() noexcept;

template <typename Derived>
class CliBase {
protected:
  // NOLINTBEGIN(*-non-private-member-variables-in-classes)
  std::string_view name;
  std::string_view desc;
  // NOLINTEND(*-non-private-member-variables-in-classes)

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
  // NOLINTBEGIN(*-non-private-member-variables-in-classes)
  std::string_view shortName;
  bool isHidden = false;
  // NOLINTEND(*-non-private-member-variables-in-classes)

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
  friend std::string formatOpts(
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

  std::string
  format(std::size_t maxShortSize, std::size_t maxOffset) const noexcept;
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
  std::string format(std::size_t maxOffset) const noexcept;
};

class Subcmd : public CliBase<Subcmd>, public ShortAndHidden<Subcmd> {
  friend class Cli;

  using MainFn = Result<void>(std::span<const std::string_view>);

  std::string_view cmdName;
  std::optional<std::vector<Opt>> globalOpts = std::nullopt;
  std::vector<Opt> localOpts;
  Arg arg;
  std::function<MainFn> mainFn;

public:
  using CliBase::CliBase;

  constexpr Subcmd& setArg(Arg arg) noexcept {
    this->arg = arg;
    return *this;
  }

  Subcmd& addOpt(Opt opt) noexcept;
  Subcmd& setMainFn(std::function<MainFn> mainFn) noexcept;
  [[nodiscard]] Result<void> noSuchArg(std::string_view arg) const;
  [[nodiscard]] static Result<void> missingOptArgument(std::string_view arg
  ) noexcept;

private:
  constexpr bool hasShort() const noexcept {
    return !shortName.empty();
  }
  constexpr Subcmd& setCmdName(std::string_view cmdName) noexcept {
    this->cmdName = cmdName;
    return *this;
  }

  Subcmd& setGlobalOpts(const std::vector<Opt>& globalOpts) noexcept;
  std::string formatUsage(std::ostream& os) const noexcept;
  std::string formatHelp() const noexcept;
  std::string format(std::size_t maxOffset) const noexcept;

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

  [[nodiscard]] Result<void> noSuchArg(std::string_view arg) const;
  [[nodiscard]] Result<void>
  exec(std::string_view subcmd, std::span<const std::string_view> args) const;
  void printSubcmdHelp(std::string_view subcmd) const noexcept;
  [[nodiscard]] Result<void> printHelp(std::span<const std::string_view> args
  ) const noexcept;
  std::size_t calcMaxOffset(std::size_t maxShortSize) const noexcept;
  std::string
  formatAllSubcmds(bool showHidden, std::size_t maxOffset = 0) const noexcept;

  enum class ControlFlow : std::uint8_t {
    Return,
    Continue,
    Fallthrough,
  };
  using enum ControlFlow;

  [[nodiscard]] static Result<ControlFlow> handleGlobalOpts(
      std::forward_iterator auto& itr, const std::forward_iterator auto end,
      std::string_view subcmd = ""
  ) {
    using std::string_view_literals::operator""sv;

    if (*itr == "-h"sv || *itr == "--help"sv) {
      if (!subcmd.empty()) {
        // {{ }} is a workaround for std::span until C++26.
        return getCli().printHelp({ { subcmd } }).map([] { return Return; });
      } else {
        return getCli().printHelp({}).map([] { return Return; });
      }
    } else if (*itr == "-v"sv || *itr == "--verbose"sv) {
      logger::setLevel(logger::Level::Debug);
      return Ok(Continue);
    } else if (*itr == "-vv"sv) {
      logger::setLevel(logger::Level::Trace);
      return Ok(Continue);
    } else if (*itr == "-q"sv || *itr == "--quiet"sv) {
      logger::setLevel(logger::Level::Off);
      return Ok(Continue);
    } else if (*itr == "--color"sv) {
      Ensure(itr + 1 < end, "missing argument for `--color`");
      setColorMode(*++itr);
      return Ok(Continue);
    }
    return Ok(Fallthrough);
  }

private:
  std::vector<std::string_view> transformOptions(
      std::string_view subcmd, std::span<const std::string_view> args
  ) const;

  std::size_t calcMaxShortSize() const noexcept;

  /// Format help message for cabin itself.
  std::string formatCmdHelp() const noexcept;
};

}  // namespace cabin
