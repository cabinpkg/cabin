#include "Semver.hpp"

#include <cctype>
#include <cstddef>
#include <cstdint>
#include <limits>
#include <ostream>
#include <sstream>
#include <string>
#include <string_view>
#include <vector>

std::ostream&
operator<<(std::ostream& os, const VersionToken& tok) noexcept {
  switch (tok.kind) {
    case VersionToken::Num:
      os << std::get<uint64_t>(tok.value);
      break;
    case VersionToken::Ident:
      os << std::get<std::string_view>(tok.value);
      break;
    case VersionToken::Dot:
      os << '.';
      break;
    case VersionToken::Hyphen:
      os << '-';
      break;
    case VersionToken::Plus:
      os << '+';
      break;
    case VersionToken::Eof:
    case VersionToken::Unknown:
      break;
  }
  return os;
}

std::string
VersionToken::toString() const noexcept {
  std::ostringstream oss;
  oss << *this;
  return oss.str();
}

std::size_t
VersionToken::size() const noexcept {
  return toString().size();
}

bool
operator==(const VersionToken& lhs, const VersionToken& rhs) noexcept {
  if (lhs.kind != rhs.kind) {
    return false;
  }
  switch (lhs.kind) {
    case VersionToken::Num:
      return std::get<uint64_t>(lhs.value) == std::get<uint64_t>(rhs.value);
    case VersionToken::Ident:
      return std::get<std::string_view>(lhs.value)
             == std::get<std::string_view>(rhs.value);
    case VersionToken::Dot:
    case VersionToken::Hyphen:
    case VersionToken::Plus:
    case VersionToken::Eof:
    case VersionToken::Unknown:
      return true;
  }
  return false;
}

bool
operator<(const VersionToken& lhs, const VersionToken& rhs) noexcept {
  if (lhs.kind == VersionToken::Num && rhs.kind == VersionToken::Num) {
    return std::get<uint64_t>(lhs.value) < std::get<uint64_t>(rhs.value);
  }
  return lhs.toString() < rhs.toString();
}
bool
operator>(const VersionToken& lhs, const VersionToken& rhs) {
  return rhs < lhs;
}

static std::string
carets(const VersionToken& tok) noexcept {
  switch (tok.kind) {
    case VersionToken::Eof:
    case VersionToken::Unknown:
      return "^";
    default:
      // NOLINTNEXTLINE(modernize-return-braced-init-list)
      return std::string(tok.size(), '^');
  }
}

bool
Prerelease::empty() const noexcept {
  return ident.empty();
}

std::string
Prerelease::toString() const noexcept {
  std::string str;
  for (std::size_t i = 0; i < ident.size(); ++i) {
    if (i > 0) {
      str += '.';
    }
    str += ident[i].toString();
  }
  return str;
}

bool
operator==(const Prerelease& lhs, const Prerelease& rhs) noexcept {
  return lhs.ident == rhs.ident;
}
bool
operator!=(const Prerelease& lhs, const Prerelease& rhs) noexcept {
  return !(lhs == rhs);
}
bool
operator<(const Prerelease& lhs, const Prerelease& rhs) noexcept {
  if (lhs.ident.empty()) {
    return false;  // lhs is a normal version and is greater
  }
  if (rhs.ident.empty()) {
    return true;  // rhs is a normal version and is greater
  }
  for (std::size_t i = 0; i < lhs.ident.size() && i < rhs.ident.size(); ++i) {
    if (lhs.ident[i] < rhs.ident[i]) {
      return true;
    } else if (lhs.ident[i] > rhs.ident[i]) {
      return false;
    }
  }
  return lhs.ident.size() < rhs.ident.size();
}
bool
operator>(const Prerelease& lhs, const Prerelease& rhs) noexcept {
  return rhs < lhs;
}
bool
operator<=(const Prerelease& lhs, const Prerelease& rhs) noexcept {
  return !(rhs < lhs);
}
bool
operator>=(const Prerelease& lhs, const Prerelease& rhs) noexcept {
  return !(lhs < rhs);
}

bool
BuildMetadata::empty() const noexcept {
  return ident.empty();
}

std::string
BuildMetadata::toString() const noexcept {
  std::string str;
  for (std::size_t i = 0; i < ident.size(); ++i) {
    if (i > 0) {
      str += '.';
    }
    str += ident[i].toString();
  }
  return str;
}

bool
operator==(const BuildMetadata& lhs, const BuildMetadata& rhs) noexcept {
  return lhs.ident == rhs.ident;
}
bool
operator<(const BuildMetadata& lhs, const BuildMetadata& rhs) noexcept {
  for (std::size_t i = 0; i < lhs.ident.size() && i < rhs.ident.size(); ++i) {
    if (lhs.ident[i] < rhs.ident[i]) {
      return true;
    } else if (lhs.ident[i] > rhs.ident[i]) {
      return false;
    }
  }
  return lhs.ident.size() < rhs.ident.size();
}
bool
operator>(const BuildMetadata& lhs, const BuildMetadata& rhs) noexcept {
  return rhs < lhs;
}

std::string
Version::toString() const noexcept {
  std::string str = std::to_string(major);
  str += '.' + std::to_string(minor);
  str += '.' + std::to_string(patch);
  if (!pre.empty()) {
    str += '-';
    str += pre.toString();
  }
  if (!build.empty()) {
    str += '+';
    str += build.toString();
  }
  return str;
}

std::ostream&
operator<<(std::ostream& os, const Version& ver) noexcept {
  os << ver.toString();
  return os;
}

bool
operator==(const Version& lhs, const Version& rhs) noexcept {
  return lhs.major == rhs.major && lhs.minor == rhs.minor
         && lhs.patch == rhs.patch && lhs.pre == rhs.pre
         && lhs.build == rhs.build;
}
bool
operator!=(const Version& lhs, const Version& rhs) noexcept {
  return !(lhs == rhs);
}

bool
operator<(const Version& lhs, const Version& rhs) noexcept {
  if (lhs.major < rhs.major) {
    return true;
  } else if (lhs.major > rhs.major) {
    return false;
  } else if (lhs.minor < rhs.minor) {
    return true;
  } else if (lhs.minor > rhs.minor) {
    return false;
  } else if (lhs.patch < rhs.patch) {
    return true;
  } else if (lhs.patch > rhs.patch) {
    return false;
  } else if (!lhs.pre.empty() && rhs.pre.empty()) {
    // lhs has a pre-release tag and rhs doesn't, so lhs < rhs
    return true;
  } else if (lhs.pre.empty() && !rhs.pre.empty()) {
    // lhs doesn't have a pre-release tag and rhs does, so lhs > rhs
    return false;
  } else if (lhs.pre < rhs.pre) {
    // Both lhs and rhs have pre-release tags, so compare them
    return true;
  } else if (lhs.pre > rhs.pre) {
    return false;
  } else if (lhs.build < rhs.build) {
    return true;
  } else if (lhs.build > rhs.build) {
    return false;
  } else {
    return false;
  }
}
bool
operator>(const Version& lhs, const Version& rhs) noexcept {
  return rhs < lhs;
}
bool
operator<=(const Version& lhs, const Version& rhs) noexcept {
  return !(rhs < lhs);
}
bool
operator>=(const Version& lhs, const Version& rhs) noexcept {
  return !(lhs < rhs);
}

VersionToken
VersionLexer::consumeIdent() noexcept {
  std::size_t len = 0;
  while (pos < s.size() && (std::isalnum(s[pos]) || s[pos] == '-')) {
    step();
    ++len;
  }
  return { VersionToken::Ident, std::string_view(s.data() + pos - len, len) };
}

VersionToken
VersionLexer::consumeNum() {
  std::size_t len = 0;
  uint64_t value = 0;
  while (pos < s.size() && std::isdigit(s[pos])) {
    if (len > 0 && value == 0) {
      throw SemverError(
          s, '\n', std::string(pos - len, ' '), "^ invalid leading zero"
      );
    }

    const uint64_t digit = s[pos] - '0';
    constexpr uint64_t base = 10;
    // Check for overflow
    if (value > (std::numeric_limits<uint64_t>::max() - digit) / base) {
      throw SemverError(
          s, '\n', std::string(pos - len, ' '), std::string(len, '^'),
          " number exceeds UINT64_MAX"
      );
    }

    value = value * base + digit;
    step();
    ++len;
  }
  return { VersionToken::Num, value };
}

// Note that 012 is an invalid number but 012d is a valid identifier.
VersionToken
VersionLexer::consumeNumOrIdent() {
  const std::size_t oldPos = pos;  // we need two passes
  bool isIdent = false;
  while (pos < s.size() && (std::isalnum(s[pos]) || s[pos] == '-')) {
    if (!std::isdigit(s[pos])) {
      isIdent = true;
    }
    step();
  }

  pos = oldPos;
  if (isIdent) {
    return consumeIdent();
  } else {
    return consumeNum();
  }
}

VersionToken
VersionLexer::next() {
  if (isEof()) {
    return VersionToken{ VersionToken::Eof };
  }

  const char c = s[pos];
  if (std::isalpha(c)) {
    return consumeIdent();
  } else if (std::isdigit(c)) {
    return consumeNumOrIdent();
  } else if (c == '.') {
    step();
    return VersionToken{ VersionToken::Dot };
  } else if (c == '-') {
    step();
    return VersionToken{ VersionToken::Hyphen };
  } else if (c == '+') {
    step();
    return VersionToken{ VersionToken::Plus };
  } else {
    step();
    return VersionToken{ VersionToken::Unknown };
  }
}

VersionToken
VersionLexer::peek() {
  const std::size_t oldPos = pos;
  const VersionToken tok = next();
  pos = oldPos;
  return tok;
}

struct SemverParseError : public SemverError {
  SemverParseError(
      const VersionLexer& lexer, const VersionToken& tok,
      const std::string_view msg
  )
      : SemverError(
            lexer.s, '\n', std::string(lexer.pos, ' '), carets(tok), msg
        ) {}
};

Version
VersionParser::parse() {
  if (lexer.peek().kind == VersionToken::Eof) {
    throw SemverError("empty string is not a valid semver");
  }

  Version ver;
  ver.major = parseNum();
  parseDot();
  ver.minor = parseNum();
  parseDot();
  ver.patch = parseNum();

  if (lexer.peek().kind == VersionToken::Hyphen) {
    lexer.step();
    ver.pre = parsePre();
  } else {
    ver.pre = Prerelease();
  }

  if (lexer.peek().kind == VersionToken::Plus) {
    lexer.step();
    ver.build = parseBuild();
  } else {
    ver.build = BuildMetadata();
  }

  if (!lexer.isEof()) {
    throw SemverParseError(
        lexer, lexer.peek(),
        " unexpected character: `" + std::string(1, lexer.s[lexer.pos]) + '`'
    );
  }

  return ver;
}

// Even if the token can be parsed as an identifier, try to parse it as a
// number.
uint64_t
VersionParser::parseNum() {
  if (!std::isdigit(lexer.s[lexer.pos])) {
    throw SemverParseError(lexer, lexer.peek(), " expected number");
  }
  return std::get<uint64_t>(lexer.consumeNum().value);
}

void
VersionParser::parseDot() {
  const VersionToken tok = lexer.next();
  if (tok.kind != VersionToken::Dot) {
    throw SemverParseError(lexer, tok, " expected `.`");
  }
}

// pre ::= numOrIdent ("." numOrIdent)*
Prerelease
VersionParser::parsePre() {
  std::vector<VersionToken> pre;
  pre.emplace_back(parseNumOrIdent());
  while (lexer.peek().kind == VersionToken::Dot) {
    lexer.step();
    pre.emplace_back(parseNumOrIdent());
  }
  return Prerelease{ pre };
}

// numOrIdent ::= num | ident
VersionToken
VersionParser::parseNumOrIdent() {
  const VersionToken tok = lexer.next();
  if (tok.kind != VersionToken::Num && tok.kind != VersionToken::Ident) {
    throw SemverParseError(lexer, tok, " expected number or identifier");
  }
  return tok;
}

// build ::= ident ("." ident)*
BuildMetadata
VersionParser::parseBuild() {
  std::vector<VersionToken> build;
  build.emplace_back(parseIdent());
  while (lexer.peek().kind == VersionToken::Dot) {
    lexer.step();
    build.emplace_back(parseIdent());
  }
  return BuildMetadata{ build };
}

// Even if the token can be parsed as a number, try to parse it as an
// identifier.
VersionToken
VersionParser::parseIdent() {
  if (!std::isalnum(lexer.s[lexer.pos])) {
    throw SemverParseError(lexer, lexer.peek(), " expected identifier");
  }
  return lexer.consumeIdent();
}

Prerelease
Prerelease::parse(const std::string_view str) {
  VersionParser parser(str);
  return parser.parsePre();
}
Result<Prerelease>
Prerelease::tryParse(const std::string_view str) noexcept {
  try {
    return Ok(parse(str));
  } catch (const SemverError& e) {
    Bail(e.what());
  }
}

BuildMetadata
BuildMetadata::parse(const std::string_view str) {
  VersionParser parser(str);
  return parser.parseBuild();
}
Result<BuildMetadata>
BuildMetadata::tryParse(const std::string_view str) noexcept {
  try {
    return Ok(parse(str));
  } catch (const SemverError& e) {
    Bail(e.what());
  }
}

Version
Version::parse(const std::string_view str) {
  VersionParser parser(str);
  return parser.parse();
}
Result<Version>
Version::tryParse(const std::string_view str) noexcept {
  try {
    return Ok(parse(str));
  } catch (const SemverError& e) {
    Bail(e.what());
  }
}

// FIXME: remove exceptions and use Result entirely.

#ifdef CABIN_TEST

#  include "Rustify/Tests.hpp"

namespace tests {

// Thanks to:
// https://github.com/dtolnay/semver/blob/55fa2cadd6ec95be02e5a2a87b24355304e44d40/tests/test_version.rs#L13

static void
testParse() {
  assertException<SemverError>(
      []() { Version::parse(""); },
      "invalid semver:\n"
      "empty string is not a valid semver"
  );
  assertException<SemverError>(
      []() { Version::parse("  "); },
      "invalid semver:\n"
      "  \n"
      "^ expected number"
  );
  assertException<SemverError>(
      []() { Version::parse("1"); },
      "invalid semver:\n"
      "1\n"
      " ^ expected `.`"
  );
  assertException<SemverError>(
      []() { Version::parse("1.2"); },
      "invalid semver:\n"
      "1.2\n"
      "   ^ expected `.`"
  );
  assertException<SemverError>(
      []() { Version::parse("1.2.3-"); },
      "invalid semver:\n"
      "1.2.3-\n"
      "      ^ expected number or identifier"
  );
  assertException<SemverError>(
      []() { Version::parse("00"); },
      "invalid semver:\n"
      "00\n"
      "^ invalid leading zero"
  );
  assertException<SemverError>(
      []() { Version::parse("0.00.0"); },
      "invalid semver:\n"
      "0.00.0\n"
      "  ^ invalid leading zero"
  );
  assertException<SemverError>(
      []() { Version::parse("0.0.0.0"); },
      "invalid semver:\n"
      "0.0.0.0\n"
      "     ^ unexpected character: `.`"
  );
  assertException<SemverError>(
      []() { Version::parse("a.b.c"); },
      "invalid semver:\n"
      "a.b.c\n"
      "^ expected number"
  );
  assertException<SemverError>(
      []() { Version::parse("1.2.3 abc"); },
      "invalid semver:\n"
      "1.2.3 abc\n"
      "     ^ unexpected character: ` `"
  );
  assertException<SemverError>(
      []() { Version::parse("1.2.3-01"); },
      "invalid semver:\n"
      "1.2.3-01\n"
      "      ^ invalid leading zero"
  );
  assertException<SemverError>(
      []() { Version::parse("1.2.3++"); },
      "invalid semver:\n"
      "1.2.3++\n"
      "      ^ expected identifier"
  );
  assertException<SemverError>(
      []() { Version::parse("07"); },
      "invalid semver:\n"
      "07\n"
      "^ invalid leading zero"
  );
  assertException<SemverError>(
      []() { Version::parse("111111111111111111111.0.0"); },
      "invalid semver:\n"
      "111111111111111111111.0.0\n"
      "^^^^^^^^^^^^^^^^^^^^ number exceeds UINT64_MAX"
  );
  assertException<SemverError>(
      []() { Version::parse("0.99999999999999999999999.0"); },
      "invalid semver:\n"
      "0.99999999999999999999999.0\n"
      "  ^^^^^^^^^^^^^^^^^^^ number exceeds UINT64_MAX"
  );
  assertException<SemverError>(
      []() { Version::parse("8\0"); },
      "invalid semver:\n"
      "8\n"
      " ^ expected `.`"
  );

  assertEq(
      Version::parse("1.2.3"), (Version{ .major = 1,
                                         .minor = 2,
                                         .patch = 3,
                                         .pre = Prerelease(),
                                         .build = BuildMetadata() })
  );
  assertEq(
      Version::parse("1.2.3-alpha1"),
      (Version{ .major = 1,
                .minor = 2,
                .patch = 3,
                .pre = Prerelease::parse("alpha1"),
                .build = BuildMetadata() })
  );
  assertEq(
      Version::parse("1.2.3+build5"),
      (Version{ .major = 1,
                .minor = 2,
                .patch = 3,
                .pre = Prerelease(),
                .build = BuildMetadata::parse("build5") })
  );
  assertEq(
      Version::parse("1.2.3+5build"),
      (Version{ .major = 1,
                .minor = 2,
                .patch = 3,
                .pre = Prerelease(),
                .build = BuildMetadata::parse("5build") })
  );
  assertEq(
      Version::parse("1.2.3-alpha1+build5"),
      (Version{ .major = 1,
                .minor = 2,
                .patch = 3,
                .pre = Prerelease::parse("alpha1"),
                .build = BuildMetadata::parse("build5") })
  );
  assertEq(
      Version::parse("1.2.3-1.alpha1.9+build5.7.3aedf"),
      (Version{ .major = 1,
                .minor = 2,
                .patch = 3,
                .pre = Prerelease::parse("1.alpha1.9"),
                .build = BuildMetadata::parse("build5.7.3aedf") })
  );
  assertEq(
      Version::parse("1.2.3-0a.alpha1.9+05build.7.3aedf"),
      (Version{ .major = 1,
                .minor = 2,
                .patch = 3,
                .pre = Prerelease::parse("0a.alpha1.9"),
                .build = BuildMetadata::parse("05build.7.3aedf") })
  );
  assertEq(
      Version::parse("0.4.0-beta.1+0851523"),
      (Version{ .major = 0,
                .minor = 4,
                .patch = 0,
                .pre = Prerelease::parse("beta.1"),
                .build = BuildMetadata::parse("0851523") })
  );
  assertEq(
      Version::parse("1.1.0-beta-10"),
      (Version{ .major = 1,
                .minor = 1,
                .patch = 0,
                .pre = Prerelease::parse("beta-10"),
                .build = BuildMetadata() })
  );

  pass();
}

static void
testEq() {
  assertEq(Version::parse("1.2.3"), Version::parse("1.2.3"));
  assertEq(Version::parse("1.2.3-alpha1"), Version::parse("1.2.3-alpha1"));
  assertEq(Version::parse("1.2.3+build.42"), Version::parse("1.2.3+build.42"));
  assertEq(
      Version::parse("1.2.3-alpha1+42"), Version::parse("1.2.3-alpha1+42")
  );

  pass();
}

static void
testNe() {
  assertNe(Version::parse("0.0.0"), Version::parse("0.0.1"));
  assertNe(Version::parse("0.0.0"), Version::parse("0.1.0"));
  assertNe(Version::parse("0.0.0"), Version::parse("1.0.0"));
  assertNe(Version::parse("1.2.3-alpha"), Version::parse("1.2.3-beta"));
  assertNe(Version::parse("1.2.3+23"), Version::parse("1.2.3+42"));

  pass();
}

static void
testDisplay() {
  {
    std::ostringstream oss;
    oss << Version::parse("1.2.3");
    assertEq(oss.str(), "1.2.3");
  }
  {
    std::ostringstream oss;
    oss << Version::parse("1.2.3-alpha1");
    assertEq(oss.str(), "1.2.3-alpha1");
  }
  {
    std::ostringstream oss;
    oss << Version::parse("1.2.3+build.42");
    assertEq(oss.str(), "1.2.3+build.42");
  }
  {
    std::ostringstream oss;
    oss << Version::parse("1.2.3-alpha1+42");
    assertEq(oss.str(), "1.2.3-alpha1+42");
  }

  pass();
}

static void
testLt() {
  assertLt(Version::parse("0.0.0"), Version::parse("1.2.3-alpha2"));
  assertLt(Version::parse("1.0.0"), Version::parse("1.2.3-alpha2"));
  assertLt(Version::parse("1.2.0"), Version::parse("1.2.3-alpha2"));
  assertLt(Version::parse("1.2.3-alpha1"), Version::parse("1.2.3"));
  assertLt(Version::parse("1.2.3-alpha1"), Version::parse("1.2.3-alpha2"));
  assertFalse(Version::parse("1.2.3-alpha2") < Version::parse("1.2.3-alpha2"));
  assertLt(Version::parse("1.2.3+23"), Version::parse("1.2.3+42"));

  pass();
}

static void
testLe() {
  assertTrue(Version::parse("0.0.0") <= Version::parse("1.2.3-alpha2"));
  assertTrue(Version::parse("1.0.0") <= Version::parse("1.2.3-alpha2"));
  assertTrue(Version::parse("1.2.0") <= Version::parse("1.2.3-alpha2"));
  assertTrue(Version::parse("1.2.3-alpha1") <= Version::parse("1.2.3-alpha2"));
  assertTrue(Version::parse("1.2.3-alpha2") <= Version::parse("1.2.3-alpha2"));
  assertTrue(Version::parse("1.2.3+23") <= Version::parse("1.2.3+42"));

  pass();
}

static void
testGt() {
  assertTrue(Version::parse("1.2.3-alpha2") > Version::parse("0.0.0"));
  assertTrue(Version::parse("1.2.3-alpha2") > Version::parse("1.0.0"));
  assertTrue(Version::parse("1.2.3-alpha2") > Version::parse("1.2.0"));
  assertTrue(Version::parse("1.2.3-alpha2") > Version::parse("1.2.3-alpha1"));
  assertTrue(Version::parse("1.2.3") > Version::parse("1.2.3-alpha2"));
  assertFalse(Version::parse("1.2.3-alpha2") > Version::parse("1.2.3-alpha2"));
  assertFalse(Version::parse("1.2.3+23") > Version::parse("1.2.3+42"));

  pass();
}

static void
testGe() {
  assertTrue(Version::parse("1.2.3-alpha2") >= Version::parse("0.0.0"));
  assertTrue(Version::parse("1.2.3-alpha2") >= Version::parse("1.0.0"));
  assertTrue(Version::parse("1.2.3-alpha2") >= Version::parse("1.2.0"));
  assertTrue(Version::parse("1.2.3-alpha2") >= Version::parse("1.2.3-alpha1"));
  assertTrue(Version::parse("1.2.3-alpha2") >= Version::parse("1.2.3-alpha2"));
  assertFalse(Version::parse("1.2.3+23") >= Version::parse("1.2.3+42"));

  pass();
}

static void
testSpecOrder() {
  const std::vector<std::string> vers = {
    "1.0.0-alpha",  "1.0.0-alpha.1", "1.0.0-alpha.beta", "1.0.0-beta",
    "1.0.0-beta.2", "1.0.0-beta.11", "1.0.0-rc.1",       "1.0.0",
  };
  for (std::size_t i = 1; i < vers.size(); ++i) {
    assertLt(Version::parse(vers[i - 1]), Version::parse(vers[i]));
  }

  pass();
}

}  // namespace tests

int
main() {
  tests::testParse();
  tests::testEq();
  tests::testNe();
  tests::testDisplay();
  tests::testLt();
  tests::testLe();
  tests::testGt();
  tests::testGe();
  tests::testSpecOrder();
}

#endif
