#pragma once

#include "Rustify/Result.hpp"

#include <cstdio>
#include <cstdlib>
#include <functional>
#include <iterator>
#include <optional>
#include <span>
#include <string>
#include <string_view>
#include <unordered_map>
#include <unordered_set>
#include <vector>

namespace cabin {

// Forward declarations
class Opt;
class Arg;
class Subcmd;
class Cli;

using CliArgsView = std::span<const std::string>;
using Opts = std::unordered_set<Opt>;

// FIXME: remove this.  To do so, we need to do actions (like printing help)
// within the Cli class in addition to just parsing the arguments.
// Defined in Cabin.cc
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

  constexpr bool hasShort() const noexcept {
    return !shortName.empty();
  }

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
  friend struct std::hash<Opt>;

  std::string_view placeholder;
  std::string_view defaultVal;
  bool isGlobal = false;

public:
  using CliBase::CliBase;

  friend void addOptCandidates(
      std::vector<std::string_view>& candidates, const Opts& opts
  ) noexcept;
  friend std::size_t calcOptMaxShortSize(const Opts& opts) noexcept;
  friend std::size_t
  calcOptMaxOffset(const Opts& opts, std::size_t maxShortSize) noexcept;
  friend std::string formatOpts(
      const Opts& opts, std::size_t maxShortSize, std::size_t maxOffset
  ) noexcept;

  constexpr Opt& setPlaceholder(const std::string_view placeholder) noexcept {
    this->placeholder = placeholder;
    return *this;
  }
  constexpr bool takesArg() const noexcept {
    return !placeholder.empty();
  }

  constexpr Opt& setDefault(const std::string_view defaultVal) noexcept {
    this->defaultVal = defaultVal;
    return *this;
  }
  constexpr Opt& setGlobal(const bool isGlobal) noexcept {
    this->isGlobal = isGlobal;
    return *this;
  }

  constexpr bool operator==(const Opt& other) const noexcept {
    return name == other.name;
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

}  // namespace cabin

template <>
struct std::hash<cabin::Opt> {
  std::size_t operator()(const cabin::Opt& opt) const noexcept {
    return std::hash<std::string_view>{}(opt.name);
  }
};

namespace cabin {

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

  using MainFn = Result<void>(CliArgsView);

  std::string_view cmdName;
  std::optional<Opts> globalOpts = std::nullopt;
  Opts localOpts;
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
  [[nodiscard]] AnyhowErr noSuchArg(std::string_view arg) const;
  [[nodiscard]] static AnyhowErr
  missingOptArgumentFor(std::string_view arg) noexcept;

private:
  constexpr Subcmd& setCmdName(std::string_view cmdName) noexcept {
    this->cmdName = cmdName;
    return *this;
  }

  Subcmd& setGlobalOpts(const Opts& globalOpts) noexcept;
  std::string formatUsage(FILE* file) const noexcept;
  std::string formatHelp() const noexcept;
  std::string format(std::size_t maxOffset) const noexcept;

  std::size_t calcMaxShortSize() const noexcept;
  /// Calculate the maximum length of the left side of the helps to align the
  /// descriptions with 2 spaces.
  std::size_t calcMaxOffset(std::size_t maxShortSize) const noexcept;
};

class Cli : public CliBase<Cli> {
  std::unordered_map<std::string_view, Subcmd> subcmds;
  Opts globalOpts;
  Opts localOpts;

public:
  using CliBase::CliBase;

  Cli& addSubcmd(const Subcmd& subcmd) noexcept;
  Cli& addOpt(Opt opt) noexcept;
  bool hasSubcmd(std::string_view subcmd) const noexcept;

  [[nodiscard]] AnyhowErr noSuchArg(std::string_view arg) const;
  [[nodiscard]] Result<void>
  exec(std::string_view subcmd, CliArgsView args) const;
  void printSubcmdHelp(std::string_view subcmd) const noexcept;
  [[nodiscard]] Result<void> printHelp(CliArgsView args) const noexcept;
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
      std::forward_iterator auto& itr, std::forward_iterator auto end,
      const std::string& subcmd = ""
  );

  // NOLINTNEXTLINE(*-avoid-c-arrays)
  Result<void> parseArgs(int argc, char* argv[]) const noexcept;

  // NOTE: This is public only for tests
  Result<std::vector<std::string>>
  expandOpts(std::span<const char* const> args) const noexcept;

private:
  Result<void> parseArgs(CliArgsView args) const noexcept;

  std::size_t calcMaxShortSize() const noexcept;

  /// Format help message for cabin itself.
  std::string formatCmdHelp() const noexcept;
};

}  // namespace cabin
