// Semver parser
//
// Syntax:
//   version    ::= num "." num "." num ("-" pre)? ("+" build)?
//   pre        ::= numOrIdent ("." numOrIdent)*
//   build      ::= ident ("." ident)*
//   numOrIdent ::= num | ident
//   num        ::= [1-9][0-9]*
//   ident      ::= [a-zA-Z0-9][a-zA-Z0-9-]*
#pragma once

#include <cstddef>
#include <cstdint>
#include <format>
#include <ostream>
#include <rs/result.hpp>
#include <string>
#include <string_view>
#include <utility>
#include <variant>
#include <vector>

struct VersionToken {
  enum class Kind : uint8_t {
    Num,    // [1-9][0-9]*
    Ident,  // [a-zA-Z0-9][a-zA-Z0-9-]*
    Dot,    // .
    Hyphen, // -
    Plus,   // +
    Eof,
    Unknown,
  };
  using enum Kind;

  Kind kind;
  std::variant<std::monostate, uint64_t, std::string> value;

  VersionToken(
      const Kind kind,
      std::variant<std::monostate, uint64_t, std::string> value) noexcept
      : kind(kind), value(std::move(value)) {}
  constexpr explicit VersionToken(const Kind kind) noexcept
      : kind(kind), value(std::monostate{}) {}

  std::string toString() const noexcept;
  std::size_t size() const noexcept;
};

struct Prerelease {
  std::vector<VersionToken> ident;

  static rs::Result<Prerelease> parse(std::string_view str) noexcept;
  bool empty() const noexcept;
  std::string toString() const noexcept;
};
bool operator==(const Prerelease& lhs, const Prerelease& rhs) noexcept;
bool operator!=(const Prerelease& lhs, const Prerelease& rhs) noexcept;
bool operator<(const Prerelease& lhs, const Prerelease& rhs) noexcept;
bool operator>(const Prerelease& lhs, const Prerelease& rhs) noexcept;
bool operator<=(const Prerelease& lhs, const Prerelease& rhs) noexcept;
bool operator>=(const Prerelease& lhs, const Prerelease& rhs) noexcept;

struct BuildMetadata {
  std::vector<VersionToken> ident;

  static rs::Result<BuildMetadata> parse(std::string_view str) noexcept;
  bool empty() const noexcept;
  std::string toString() const noexcept;
};

struct Version {
  uint64_t major{};
  uint64_t minor{};
  uint64_t patch{};
  Prerelease pre;
  BuildMetadata build;

  static rs::Result<Version> parse(std::string_view str) noexcept;
  std::string toString() const noexcept;
  uint64_t toNum() const noexcept;
};
std::ostream& operator<<(std::ostream& os, const Version& ver) noexcept;
bool operator==(const Version& lhs, const Version& rhs) noexcept;
bool operator!=(const Version& lhs, const Version& rhs) noexcept;
bool operator<(const Version& lhs, const Version& rhs) noexcept;
bool operator>(const Version& lhs, const Version& rhs) noexcept;
bool operator<=(const Version& lhs, const Version& rhs) noexcept;
bool operator>=(const Version& lhs, const Version& rhs) noexcept;

struct VersionLexer {
  std::string_view s;
  std::size_t pos{ 0 };

  constexpr explicit VersionLexer(const std::string_view str) noexcept
      : s(str) {}

  constexpr bool isEof() const noexcept { return pos >= s.size(); }
  constexpr void step() noexcept { ++pos; }
  VersionToken consumeIdent() noexcept;
  rs::Result<VersionToken> consumeNum() noexcept;
  rs::Result<VersionToken> consumeNumOrIdent() noexcept;
  rs::Result<VersionToken> next() noexcept;
  rs::Result<VersionToken> peek() noexcept;
  constexpr char curChar() const noexcept { return isEof() ? '\0' : s[pos]; }
};

struct VersionParser {
  VersionLexer lexer;

  constexpr explicit VersionParser(const std::string_view str) noexcept
      : lexer(str) {}

  rs::Result<Version> parse() noexcept;
  rs::Result<uint64_t> parseNum() noexcept;
  rs::Result<void> parseDot() noexcept;
  rs::Result<Prerelease> parsePre() noexcept;
  rs::Result<VersionToken> parseNumOrIdent() noexcept;
  rs::Result<BuildMetadata> parseBuild() noexcept;
  rs::Result<VersionToken> parseIdent() noexcept;
};

namespace std {

template <>
struct formatter<Version> {
  constexpr auto parse(format_parse_context& ctx) { return ctx.begin(); }

  template <class FormatContext>
  auto format(const Version& ver, FormatContext& ctx) const {
    return formatter<std::string_view>{}.format(ver.toString(), ctx);
  }
};

} // namespace std
