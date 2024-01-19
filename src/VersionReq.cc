#include "VersionReq.hpp"

#include "Exception.hpp"
#include "Rustify.hpp"

#include <cctype>
#include <iostream>
#include <utility>
#include <variant>

struct ComparatorError : public PoacError {
  template <typename... Args>
  explicit ComparatorError(Args&&... args)
      : PoacError("invalid comparator:\n", std::forward<Args>(args)...) {}
};

struct VersionReqError : public PoacError {
  template <typename... Args>
  explicit VersionReqError(Args&&... args)
      : PoacError(
          "invalid version requirement:\n", std::forward<Args>(args)...
      ) {}
};

String
toString(const Comparator::Op op) noexcept {
  switch (op) {
    case Comparator::Exact:
      return "=";
    case Comparator::Gt:
      return ">";
    case Comparator::Gte:
      return ">=";
    case Comparator::Lt:
      return "<";
    case Comparator::Lte:
      return "<=";
  }
  unreachable();
}

struct ComparatorToken {
  enum Kind {
    Eq, // =
    Gt, // >
    Gte, // >=
    Lt, // <
    Lte, // <=
    Ver, // OptVersion
    Eof,
    Unknown,
  };

  Kind kind;
  std::variant<std::monostate, OptVersion> value;

  ComparatorToken(
      Kind kind, std::variant<std::monostate, OptVersion> value
  ) noexcept
      : kind(kind), value(std::move(value)) {}

  explicit ComparatorToken(Kind kind) noexcept
      : kind(kind), value(std::monostate{}) {}
};

struct ComparatorLexer {
  StringRef s;
  usize pos{ 0 };

  explicit ComparatorLexer(const StringRef s) noexcept : s(s) {}

  bool isEof() const noexcept {
    return pos >= s.size();
  }

  void step() noexcept {
    ++pos;
  }

  void skipWs() noexcept {
    while (!isEof() && std::isspace(s[pos])) {
      step();
    }
  }

  ComparatorToken next() {
    if (isEof()) {
      return ComparatorToken{ ComparatorToken::Eof };
    }

    const char c = s[pos];
    if (c == '=') {
      step();
      return ComparatorToken{ ComparatorToken::Eq };
    } else if (c == '>') {
      step();
      if (isEof()) {
        return ComparatorToken{ ComparatorToken::Gt };
      } else if (s[pos] == '=') {
        step();
        return ComparatorToken{ ComparatorToken::Gte };
      } else {
        return ComparatorToken{ ComparatorToken::Gt };
      }
    } else if (c == '<') {
      step();
      if (isEof()) {
        return ComparatorToken{ ComparatorToken::Lt };
      } else if (s[pos] == '=') {
        step();
        return ComparatorToken{ ComparatorToken::Lte };
      } else {
        return ComparatorToken{ ComparatorToken::Lt };
      }
    } else if (std::isdigit(c)) {
      VersionParser parser(s);
      parser.lexer.pos = pos;

      OptVersion ver;
      ver.major = parser.parseNum();
      if (parser.lexer.s[parser.lexer.pos] != '.') {
        pos = parser.lexer.pos;
        return ComparatorToken{ ComparatorToken::Ver, std::move(ver) };
      }

      parser.parseDot();
      ver.minor = parser.parseNum();
      if (parser.lexer.s[parser.lexer.pos] != '.') {
        pos = parser.lexer.pos;
        return ComparatorToken{ ComparatorToken::Ver, std::move(ver) };
      }

      parser.parseDot();
      ver.patch = parser.parseNum();

      if (parser.lexer.s[parser.lexer.pos] == '-') {
        parser.lexer.step();
        ver.pre = parser.parsePre();
      }

      if (parser.lexer.s[parser.lexer.pos] == '+') {
        parser.lexer.step();
        parser.parseBuild(); // discard build metadata
      }

      pos = parser.lexer.pos;
      return ComparatorToken{ ComparatorToken::Ver, std::move(ver) };
    } else {
      return ComparatorToken{ ComparatorToken::Unknown };
    }
  }
};

struct ComparatorParser {
  ComparatorLexer lexer;

  explicit ComparatorParser(const StringRef s) noexcept : lexer(s) {}

  Comparator parse() {
    Comparator result;

    const auto token = lexer.next();
    switch (token.kind) {
      case ComparatorToken::Eq:
        result.op = Comparator::Exact;
        break;
      case ComparatorToken::Gt:
        result.op = Comparator::Gt;
        break;
      case ComparatorToken::Gte:
        result.op = Comparator::Gte;
        break;
      case ComparatorToken::Lt:
        result.op = Comparator::Lt;
        break;
      case ComparatorToken::Lte:
        result.op = Comparator::Lte;
        break;
      case ComparatorToken::Ver:
        result.from(std::get<OptVersion>(token.value));
        break;
      default:
        throw ComparatorError(
            lexer.s, '\n', String(lexer.pos, ' '),
            "^ expected =, >=, <=, >, <, or version"
        );
    }

    // If the first token was comparison operator, the next token must be
    // version.
    if (token.kind != ComparatorToken::Ver) {
      lexer.skipWs();
      const auto token2 = lexer.next();
      if (token2.kind != ComparatorToken::Ver) {
        throw ComparatorError(
            lexer.s, '\n', String(lexer.pos, ' '), "^ expected version"
        );
      }
      result.from(std::get<OptVersion>(token2.value));
    }

    return result;
  }
};

Comparator
Comparator::parse(const StringRef s) {
  ComparatorParser parser(s);
  return parser.parse();
}

void
Comparator::from(const OptVersion& ver) noexcept {
  major = ver.major;
  minor = ver.minor;
  patch = ver.patch;
  pre = ver.pre;
}

void
optVersionString(const Comparator& cmp, String& result) noexcept {
  result += std::to_string(cmp.major);
  if (cmp.minor.has_value()) {
    result += ".";
    result += std::to_string(cmp.minor.value());

    if (cmp.patch.has_value()) {
      result += ".";
      result += std::to_string(cmp.patch.value());

      if (!cmp.pre.empty()) {
        result += "-";
        result += cmp.pre.toString();
      }
    }
  }
}

String
Comparator::toString() const noexcept {
  String result;
  if (op.has_value()) {
    result += ::toString(op.value());
  }
  optVersionString(*this, result);
  return result;
}

String
Comparator::toPkgConfigString() const noexcept {
  String result;
  if (op.has_value()) {
    result += ::toString(op.value());
    result += ' '; // we just need this space for pkg-config
  }
  optVersionString(*this, result);
  return result;
}

static bool
matchesExact(const Comparator& cmp, const Version& ver) noexcept {
  if (ver.major != cmp.major) {
    return false;
  }

  if (const auto minor = cmp.minor) {
    if (ver.minor != minor.value()) {
      return false;
    }
  }

  if (const auto patch = cmp.patch) {
    if (ver.patch != patch.value()) {
      return false;
    }
  }

  return ver.pre == cmp.pre;
}

static bool
matchesGreater(const Comparator& cmp, const Version& ver) noexcept {
  if (ver.major != cmp.major) {
    return ver.major > cmp.major;
  }

  if (!cmp.minor.has_value()) {
    return false;
  } else {
    const u64 minor = cmp.minor.value();
    if (ver.minor != minor) {
      return ver.minor > minor;
    }
  }

  if (!cmp.patch.has_value()) {
    return false;
  } else {
    const u64 patch = cmp.patch.value();
    if (ver.patch != patch) {
      return ver.patch > patch;
    }
  }

  return ver.pre > cmp.pre;
}

static bool
matchesLess(const Comparator& cmp, const Version& ver) noexcept {
  if (ver.major != cmp.major) {
    return ver.major < cmp.major;
  }

  if (!cmp.minor.has_value()) {
    return false;
  } else {
    const u64 minor = cmp.minor.value();
    if (ver.minor != minor) {
      return ver.minor < minor;
    }
  }

  if (!cmp.patch.has_value()) {
    return false;
  } else {
    const u64 patch = cmp.patch.value();
    if (ver.patch != patch) {
      return ver.patch < patch;
    }
  }

  return ver.pre < cmp.pre;
}

static bool
matchesNoOp(const Comparator& cmp, const Version& ver) noexcept {
  if (ver.major != cmp.major) {
    return false;
  }

  if (!cmp.minor.has_value()) {
    return true;
  }
  const u64 minor = cmp.minor.value();

  if (!cmp.patch.has_value()) {
    if (cmp.major > 0) {
      return ver.minor >= minor;
    } else {
      return ver.minor == minor;
    }
  }
  const u64 patch = cmp.patch.value();

  if (cmp.major > 0) {
    if (ver.minor != minor) {
      return ver.minor > minor;
    } else if (ver.patch != patch) {
      return ver.patch > patch;
    }
  } else if (minor > 0) {
    if (ver.minor != minor) {
      return false;
    } else if (ver.patch != patch) {
      return ver.patch > patch;
    }
  } else if (ver.minor != minor || ver.patch != patch) {
    return false;
  }

  return ver.pre >= cmp.pre;
}

bool
Comparator::satisfiedBy(const Version& ver) const noexcept {
  if (!op.has_value()) { // NoOp
    return matchesNoOp(*this, ver);
  }

  switch (op.value()) {
    case Op::Exact:
      return matchesExact(*this, ver);
    case Op::Gt:
      return matchesGreater(*this, ver);
    case Op::Gte:
      return matchesExact(*this, ver) || matchesGreater(*this, ver);
    case Op::Lt:
      return matchesLess(*this, ver);
    case Op::Lte:
      return matchesExact(*this, ver) || matchesLess(*this, ver);
  }
  unreachable();
}

Comparator
Comparator::canonicalize() const noexcept {
  if (!op.has_value() || op.value() == Op::Exact) {
    // For NoOp or Exact, canonicalization can be done over VersionReq.
    return *this;
  }

  Comparator cmp = *this;
  const Op op = this->op.value();
  switch (op) {
    case Op::Gt:
      cmp.op = Op::Gte;
      break;
    case Op::Lte:
      cmp.op = Op::Lt;
      break;
    default:
      cmp.minor = cmp.minor.value_or(0);
      cmp.patch = cmp.patch.value_or(0);
      return cmp;
  }

  if (patch.has_value()) {
    cmp.patch = patch.value() + 1;
    return cmp;
  } else {
    cmp.patch = 0;
  }

  if (minor.has_value()) {
    cmp.minor = minor.value() + 1;
    return cmp;
  } else {
    cmp.minor = 0;
  }

  cmp.major += 1;
  return cmp;
}

struct VersionReqToken {
  enum Kind {
    Comp,
    And,
    Eof,
    Unknown,
  };

  Kind kind;
  std::variant<std::monostate, Comparator> value;

  VersionReqToken(
      Kind kind, std::variant<std::monostate, Comparator> value
  ) noexcept
      : kind(kind), value(std::move(value)) {}

  explicit VersionReqToken(Kind kind) noexcept
      : kind(kind), value(std::monostate{}) {}
};

constexpr bool
isCompStart(const char c) noexcept {
  return c == '=' || c == '>' || c == '<';
}

struct VersionReqLexer {
  StringRef s;
  usize pos{ 0 };

  explicit VersionReqLexer(const StringRef s) noexcept : s(s) {}

  bool isEof() const noexcept {
    return pos >= s.size();
  }

  void skipWs() noexcept {
    while (!isEof() && std::isspace(s[pos])) {
      ++pos;
    }
  }

  VersionReqToken next() {
    skipWs();
    if (isEof()) {
      return VersionReqToken{ VersionReqToken::Eof };
    }

    const char c = s[pos];
    if (isCompStart(c) || std::isdigit(c)) {
      ComparatorParser parser(s);
      parser.lexer.pos = pos;

      const Comparator comp = parser.parse();
      pos = parser.lexer.pos;

      return VersionReqToken{ VersionReqToken::Comp, comp };
    } else if (c == '&' && pos + 1 < s.size() && s[pos + 1] == '&') {
      pos += 2;
      return VersionReqToken{ VersionReqToken::And };
    }

    return VersionReqToken{ VersionReqToken::Unknown };
  }
};

struct VersionReqParser {
  VersionReqLexer lexer;

  explicit VersionReqParser(const StringRef s) noexcept : lexer(s) {}

  VersionReq parse() {
    VersionReq result;

    result.left = parseComparatorOrOptVer();
    if (!result.left.op.has_value()
        || result.left.op.value() == Comparator::Exact) { // NoOp or Exact
      lexer.skipWs();
      if (!lexer.isEof()) {
        throw VersionReqError(
            lexer.s, '\n', String(lexer.pos, ' '),
            "^ NoOp and Exact cannot chain"
        );
      }
      return result;
    }

    const VersionReqToken token = lexer.next();
    if (token.kind == VersionReqToken::Eof) {
      return result;
    } else if (token.kind != VersionReqToken::And) {
      throw VersionReqError(
          lexer.s, '\n', String(lexer.pos, ' '), "^ expected `&&`"
      );
    }

    result.right = parseComparator();
    lexer.skipWs();
    if (!lexer.isEof()) {
      throw VersionReqError(
          lexer.s, '\n', String(lexer.pos, ' '), "^ expected end of string"
      );
    }

    return result;
  }

  // Parse `("=" | CompOp)? OptVersion` or `Comparator`.
  Comparator parseComparatorOrOptVer() {
    const VersionReqToken token = lexer.next();
    if (token.kind != VersionReqToken::Comp) {
      throw VersionReqError(
          lexer.s, '\n', String(lexer.pos, ' '),
          "^ expected =, >=, <=, >, <, or version"
      );
    }
    return std::get<Comparator>(token.value);
  }

  // If the token is a NoOp or Exact comparator, throw an exception.  This
  // is because NoOp and Exact cannot chain, and the Comparator parser
  // handles both `("=" | CompOp)? OptVersion` and `Comparator` cases for
  // simplicity. That is, this method literally accepts `Comparator` defined
  // in the grammar.  Otherwise, return the comparator if the token is a
  // comparator.
  Comparator parseComparator() {
    lexer.skipWs();
    if (lexer.isEof()) {
      compExpected();
    }
    if (!isCompStart(lexer.s[lexer.pos])) {
      // NoOp cannot chain.
      compExpected();
    }
    if (lexer.s[lexer.pos] == '=') {
      // Exact cannot chain.
      compExpected();
    }

    const VersionReqToken token = lexer.next();
    if (token.kind != VersionReqToken::Comp) {
      compExpected();
    }
    return std::get<Comparator>(token.value);
  }

  [[noreturn]] void compExpected() {
    throw VersionReqError(
        lexer.s, '\n', String(lexer.pos, ' '), "^ expected >=, <=, >, or <"
    );
  }
};

VersionReq
VersionReq::parse(const StringRef s) {
  VersionReqParser parser(s);
  return parser.parse();
}

static bool
preIsCompatible(const Comparator& cmp, const Version& ver) noexcept {
  return cmp.major == ver.major && cmp.minor.has_value()
         && cmp.minor.value() == ver.minor && cmp.patch.has_value()
         && cmp.patch.value() == ver.patch && !cmp.pre.empty();
}

bool
VersionReq::satisfiedBy(const Version& ver) const noexcept {
  if (!left.satisfiedBy(ver)) {
    return false;
  }
  if (right.has_value() && !right->satisfiedBy(ver)) {
    return false;
  }

  if (ver.pre.empty()) {
    return true;
  }

  if (preIsCompatible(left, ver)) {
    return true;
  }
  if (right.has_value() && preIsCompatible(right.value(), ver)) {
    return true;
  }

  return false;
}

// 1. NoOp: (= Caret (^), "compatible" updates)
//   1.1. `A.B.C` (where A > 0) is equivalent to `>=A.B.C && <(A+1).0.0`
//   1.2. `A.B` (where A > 0 & B > 0) is equivalent to `^A.B.0` (i.e., 1.1)
//   1.3. `A` is equivalent to `=A` (i.e., 2.3)
//   1.4. `0.B.C` (where B > 0) is equivalent to `>=0.B.C && <0.(B+1).0`
//   1.5. `0.0.C` is equivalent to `=0.0.C` (i.e., 2.1)
//   1.6. `0.0` is equivalent to `=0.0` (i.e., 2.2)
static VersionReq
canonicalizeNoOp(const VersionReq& target) noexcept {
  const Comparator& left = target.left;

  if (!left.minor.has_value() && !left.patch.has_value()) {
    // {{ !B.has_value() && !C.has_value() }}
    // 1.3. `A` is equivalent to `=A` (i.e., 2.3)
    VersionReq req;
    req.left.op = Comparator::Gte;
    req.left.major = left.major;
    req.left.minor = 0;
    req.left.patch = 0;
    req.left.pre = left.pre;

    req.right = Comparator();
    req.right->op = Comparator::Lt;
    req.right->major = left.major + 1;
    req.right->minor = 0;
    req.right->patch = 0;
    req.right->pre = left.pre;

    return req;
  }
  // => {{ B.has_value() || C.has_value() }}
  // => {{ B.has_value() }} since {{ !B.has_value() && C.has_value() }} is
  //    impossible as the semver parser rejects it.

  if (left.major > 0) { // => {{ A > 0 && B.has_value() }}
    if (left.patch.has_value()) {
      // => {{ A > 0 && B.has_value() && C.has_value() }}
      // 1.1. `A.B.C` (where A > 0) is equivalent to `>=A.B.C && <(A+1).0.0`
      VersionReq req;
      req.left.op = Comparator::Gte;
      req.left.major = left.major;
      req.left.minor = left.minor.value();
      req.left.patch = left.patch.value();
      req.left.pre = left.pre;

      req.right = Comparator();
      req.right->op = Comparator::Lt;
      req.right->major = left.major + 1;
      req.right->minor = 0;
      req.right->patch = 0;
      req.right->pre = left.pre;

      return req;
    } else { // => {{ A > 0 && B.has_value() && !C.has_value() }}
      // 1.2. `A.B` (where A > 0 & B > 0) is equivalent to `^A.B.0` (i.e., 1.1)
      VersionReq req;
      req.left.op = Comparator::Gte;
      req.left.major = left.major;
      req.left.minor = left.minor.value();
      req.left.patch = 0;
      req.left.pre = left.pre;

      req.right = Comparator();
      req.right->op = Comparator::Lt;
      req.right->major = left.major + 1;
      req.right->minor = 0;
      req.right->patch = 0;
      req.right->pre = left.pre;

      return req;
    }
  }
  // => {{ A == 0 && B.has_value() }}

  if (left.minor.value() > 0) { // => {{ A == 0 && B > 0 }}
    // 1.4. `0.B.C` (where B > 0) is equivalent to `>=0.B.C && <0.(B+1).0`
    VersionReq req;
    req.left.op = Comparator::Gte;
    req.left.major = 0;
    req.left.minor = left.minor.value();
    req.left.patch = left.patch.value_or(0);
    req.left.pre = left.pre;

    req.right = Comparator();
    req.right->op = Comparator::Lt;
    req.right->major = 0;
    req.right->minor = left.minor.value() + 1;
    req.right->patch = 0;
    req.right->pre = left.pre;

    return req;
  }
  // => {{ A == 0 && B == 0 }}

  if (left.patch.has_value()) { // => {{ A == 0 && B == 0 && C.has_value() }}
    // 1.5. `0.0.C` is equivalent to `=0.0.C` (i.e., 2.1)
    VersionReq req;
    req.left.op = Comparator::Exact;
    req.left.major = 0;
    req.left.minor = 0;
    req.left.patch = left.patch.value();
    req.left.pre = left.pre;
    return req;
  }
  // => {{ A == 0 && B == 0 && !C.has_value() }}

  // 1.6. `0.0` is equivalent to `=0.0` (i.e., 2.2)
  VersionReq req;
  req.left.op = Comparator::Gte;
  req.left.major = 0;
  req.left.minor = 0;
  req.left.patch = 0;
  req.left.pre = left.pre;

  req.right = Comparator();
  req.right->op = Comparator::Lt;
  req.right->major = 0;
  req.right->minor = 1;
  req.right->patch = 0;
  req.right->pre = left.pre;

  return req;
}

// 2. Exact:
//   2.1. `=A.B.C` is exactly the version `A.B.C`
//   2.2. `=A.B` is equivalent to `>=A.B.0 && <A.(B+1).0`
//   2.3. `=A` is equivalent to `>=A.0.0 && <(A+1).0.0`
static VersionReq
canonicalizeExact(const VersionReq& req) noexcept {
  const Comparator& left = req.left;

  if (left.minor.has_value() && left.patch.has_value()) {
    // 2.1. `=A.B.C` is exactly the version A.B.C
    return req;
  } else if (left.minor.has_value()) {
    // 2.2. `=A.B` is equivalent to `>=A.B.0 && <A.(B+1).0`
    VersionReq req;
    req.left.op = Comparator::Gte;
    req.left.major = left.major;
    req.left.minor = left.minor.value();
    req.left.patch = 0;
    req.left.pre = left.pre;

    req.right = Comparator();
    req.right->op = Comparator::Lt;
    req.right->major = left.major;
    req.right->minor = left.minor.value() + 1;
    req.right->patch = 0;
    req.right->pre = left.pre;

    return req;
  } else {
    // 2.3. `=A` is equivalent to `>=A.0.0 && <(A+1).0.0`
    VersionReq req;
    req.left.op = Comparator::Gte;
    req.left.major = left.major;
    req.left.minor = 0;
    req.left.patch = 0;
    req.left.pre = left.pre;

    req.right = Comparator();
    req.right->op = Comparator::Lt;
    req.right->major = left.major + 1;
    req.right->minor = 0;
    req.right->patch = 0;
    req.right->pre = left.pre;

    return req;
  }
}

VersionReq
VersionReq::canonicalize() const noexcept {
  if (!left.op.has_value()) { // NoOp
    return canonicalizeNoOp(*this);
  } else if (left.op.value() == Comparator::Exact) {
    return canonicalizeExact(*this);
  }

  VersionReq req = *this;
  req.left = left.canonicalize();
  if (right.has_value()) {
    req.right = right->canonicalize();
  }
  return req;
}

String
VersionReq::toString() const noexcept {
  String result = left.toString();
  if (right.has_value()) {
    result += " && ";
    result += right->toString();
  }
  return result;
}

String
VersionReq::toPkgConfigString(const String& name) const noexcept {
  // For pkg-config, canonicalization is necessary.
  const VersionReq req = canonicalize();

  String result = name;
  result += ' ';
  result += req.left.toPkgConfigString();
  if (req.right.has_value()) {
    result += ", " + name + ' ';
    result += req.right->toPkgConfigString();
  }
  return result;
}

bool
VersionReq::canSimplify() const noexcept {
  // NoOp and Exact will not have two comparators, so they cannot be
  // simplified.
  if (!left.op.has_value()) { // NoOp
    return false;
  } else if (left.op.value() == Comparator::Exact) {
    return false;
  }

  if (!right.has_value()) {
    // If we have only one comparator, it cannot be simplified.
    return false;
  }

  // When we have two comparators, the right operator must not be NoOp or
  // Exact.
  if (left.op.value() == right->op.value()) {
    // If the left and right comparators have the same operator, they can
    // be merged into one comparator.
    return true;
  }

  // < and <= can be merged into one comparator.
  if (left.op.value() == Comparator::Lt
      && right->op.value() == Comparator::Lte) {
    return true;
  }
  // <= and < can be merged into one comparator.
  if (left.op.value() == Comparator::Lte
      && right->op.value() == Comparator::Lt) {
    return true;
  }

  // > and >= can be merged into one comparator.
  if (left.op.value() == Comparator::Gt
      && right->op.value() == Comparator::Gte) {
    return true;
  }
  // >= and > can be merged into one comparator.
  if (left.op.value() == Comparator::Gte
      && right->op.value() == Comparator::Gt) {
    return true;
  }

  return false;
}

std::ostream&
operator<<(std::ostream& os, const VersionReq& req) {
  return os << req.toString();
}

#ifdef POAC_TEST

#  include <span>

namespace tests {

// Thanks to:
// https://github.com/dtolnay/semver/blob/b6171889ac7e8f47ec6f12003571bdcc7f737b10/tests/test_version_req.rs

inline void
assertMatchAll(
    const VersionReq& req, const std::span<const StringRef> versions,
    const StringRef f = __builtin_FILE(), const int l = __builtin_LINE(),
    const StringRef fn = __builtin_FUNCTION()
) {
  for (const StringRef ver : versions) {
    assertTrue(req.satisfiedBy(Version::parse(ver)), "", f, l, fn);
  }
}

inline void
assertMatchNone(
    const VersionReq& req, const std::span<const StringRef> versions,
    const StringRef f = __builtin_FILE(), const int l = __builtin_LINE(),
    const StringRef fn = __builtin_FUNCTION()
) {
  for (const StringRef ver : versions) {
    assertFalse(req.satisfiedBy(Version::parse(ver)), "", f, l, fn);
  }
}

void
testBasic() {
  const auto req = VersionReq::parse("1.0.0");
  assertEq(req.toString(), "1.0.0");
  assertMatchAll(req, { { "1.0.0", "1.1.0", "1.0.1" } });
  assertMatchNone(
      req, { { "0.9.9", "0.10.0", "0.1.0", "1.0.0-pre", "1.0.1-pre" } }
  );

  pass();
}

void
testExact() {
  const auto r1 = VersionReq::parse("=1.0.0");
  assertEq(r1.toString(), "=1.0.0");
  assertMatchAll(r1, { { "1.0.0" } });
  assertMatchNone(r1, { { "1.0.1", "0.9.9", "0.10.0", "0.1.0", "1.0.0-pre" } });

  const auto r2 = VersionReq::parse("=0.9.0");
  assertEq(r2.toString(), "=0.9.0");
  assertMatchAll(r2, { { "0.9.0" } });
  assertMatchNone(r2, { { "0.9.1", "1.9.0", "0.0.9", "0.9.0-pre" } });

  const auto r3 = VersionReq::parse("=0.0.2");
  assertEq(r3.toString(), "=0.0.2");
  assertMatchAll(r3, { { "0.0.2" } });
  assertMatchNone(r3, { { "0.0.1", "0.0.3", "0.0.2-pre" } });

  const auto r4 = VersionReq::parse("=0.1.0-beta2.a");
  assertEq(r4.toString(), "=0.1.0-beta2.a");
  assertMatchAll(r4, { { "0.1.0-beta2.a" } });
  assertMatchNone(r4, { { "0.9.1", "0.1.0", "0.1.1-beta2.a", "0.1.0-beta2" } });

  const auto r5 = VersionReq::parse("=0.1.0+meta");
  assertEq(r5.toString(), "=0.1.0");
  assertMatchAll(r5, { { "0.1.0", "0.1.0+meta", "0.1.0+any" } });

  pass();
}

void
testGreaterThan() {
  const auto r1 = VersionReq::parse(">=1.0.0");
  assertEq(r1.toString(), ">=1.0.0");
  assertMatchAll(r1, { { "1.0.0", "2.0.0" } });
  assertMatchNone(r1, { { "0.1.0", "0.0.1", "1.0.0-pre", "2.0.0-pre" } });

  const auto r2 = VersionReq::parse(">=2.1.0-alpha2");
  assertEq(r2.toString(), ">=2.1.0-alpha2");
  assertMatchAll(r2, { { "2.1.0-alpha2", "2.1.0-alpha3", "2.1.0", "3.0.0" } });
  assertMatchNone(
      r2, { { "2.0.0", "2.1.0-alpha1", "2.0.0-alpha2", "3.0.0-alpha2" } }
  );

  pass();
}

void
testLessThan() {
  const auto r1 = VersionReq::parse("<1.0.0");
  assertEq(r1.toString(), "<1.0.0");
  assertMatchAll(r1, { { "0.1.0", "0.0.1" } });
  assertMatchNone(r1, { { "1.0.0", "1.0.0-beta", "1.0.1", "0.9.9-alpha" } });

  const auto r2 = VersionReq::parse("<=2.1.0-alpha2");
  assertMatchAll(r2, { { "2.1.0-alpha2", "2.1.0-alpha1", "2.0.0", "1.0.0" } });
  assertMatchNone(
      r2, { { "2.1.0", "2.2.0-alpha1", "2.0.0-alpha2", "1.0.0-alpha2" } }
  );

  const auto r3 = VersionReq::parse(">1.0.0-alpha && <1.0.0");
  assertMatchAll(r3, { { "1.0.0-beta" } });

  const auto r4 = VersionReq::parse(">1.0.0-alpha && <1.0");
  assertMatchNone(r4, { { "1.0.0-beta" } });

  const auto r5 = VersionReq::parse(">1.0.0-alpha && <1");
  assertMatchNone(r5, { { "1.0.0-beta" } });

  pass();
}

// same as caret
void
testNoOp() {
  const auto r1 = VersionReq::parse("1");
  assertMatchAll(r1, { { "1.1.2", "1.1.0", "1.2.1", "1.0.1" } });
  assertMatchNone(r1, { { "0.9.1", "2.9.0", "0.1.4" } });
  assertMatchNone(r1, { { "1.0.0-beta1", "0.1.0-alpha", "1.0.1-pre" } });

  const auto r2 = VersionReq::parse("1.1");
  assertMatchAll(r2, { { "1.1.2", "1.1.0", "1.2.1" } });
  assertMatchNone(r2, { { "0.9.1", "2.9.0", "1.0.1", "0.1.4" } });

  const auto r3 = VersionReq::parse("1.1.2");
  assertMatchAll(r3, { { "1.1.2", "1.1.4", "1.2.1" } });
  assertMatchNone(r3, { { "0.9.1", "2.9.0", "1.1.1", "0.0.1" } });
  assertMatchNone(r3, { { "1.1.2-alpha1", "1.1.3-alpha1", "2.9.0-alpha1" } });

  const auto r4 = VersionReq::parse("0.1.2");
  assertMatchAll(r4, { { "0.1.2", "0.1.4" } });
  assertMatchNone(r4, { { "0.9.1", "2.9.0", "1.1.1", "0.0.1" } });
  assertMatchNone(r4, { { "0.1.2-beta", "0.1.3-alpha", "0.2.0-pre" } });

  const auto r5 = VersionReq::parse("0.5.1-alpha3");
  assertMatchAll(
      r5, { { "0.5.1-alpha3", "0.5.1-alpha4", "0.5.1-beta", "0.5.1", "0.5.5" } }
  );
  assertMatchNone(
      r5,
      { { "0.5.1-alpha1", "0.5.2-alpha3", "0.5.5-pre", "0.5.0-pre", "0.6.0" } }
  );

  const auto r6 = VersionReq::parse("0.0.2");
  assertMatchAll(r6, { { "0.0.2" } });
  assertMatchNone(r6, { { "0.9.1", "2.9.0", "1.1.1", "0.0.1", "0.1.4" } });

  const auto r7 = VersionReq::parse("0.0");
  assertMatchAll(r7, { { "0.0.2", "0.0.0" } });
  assertMatchNone(r7, { { "0.9.1", "2.9.0", "1.1.1", "0.1.4" } });

  const auto r8 = VersionReq::parse("0");
  assertMatchAll(r8, { { "0.9.1", "0.0.2", "0.0.0" } });
  assertMatchNone(r8, { { "2.9.0", "1.1.1" } });

  const auto r9 = VersionReq::parse("1.4.2-beta.5");
  assertMatchAll(
      r9, { { "1.4.2", "1.4.3", "1.4.2-beta.5", "1.4.2-beta.6", "1.4.2-c" } }
  );
  assertMatchNone(
      r9,
      { { "0.9.9", "2.0.0", "1.4.2-alpha", "1.4.2-beta.4", "1.4.3-beta.5" } }
  );

  pass();
}

void
testMultiple() {
  const auto r1 = VersionReq::parse(">0.0.9 && <=2.5.3");
  assertEq(r1.toString(), ">0.0.9 && <=2.5.3");
  assertMatchAll(r1, { { "0.0.10", "1.0.0", "2.5.3" } });
  assertMatchNone(r1, { { "0.0.8", "2.5.4" } });

  const auto r2 = VersionReq::parse("<=0.2.0 && >=0.5.0");
  assertEq(r2.toString(), "<=0.2.0 && >=0.5.0");
  assertMatchNone(r2, { { "0.0.8", "0.3.0", "0.5.1" } });

  const auto r3 = VersionReq::parse(">=0.5.1-alpha3 && <0.6");
  assertEq(r3.toString(), ">=0.5.1-alpha3 && <0.6");
  assertMatchAll(
      r3, { { "0.5.1-alpha3", "0.5.1-alpha4", "0.5.1-beta", "0.5.1", "0.5.5" } }
  );
  assertMatchNone(
      r3, { { "0.5.1-alpha1", "0.5.2-alpha3", "0.5.5-pre", "0.5.0-pre", "0.6.0",
              "0.6.0-pre" } }
  );

  assertException<VersionReqError>(
      []() { VersionReq::parse(">0.3.0 && &&"); },
      "invalid version requirement:\n"
      ">0.3.0 && &&\n"
      "          ^ expected >=, <=, >, or <"
  );

  const auto r4 = VersionReq::parse(">=0.5.1-alpha3 && <0.6");
  assertEq(r4.toString(), ">=0.5.1-alpha3 && <0.6");
  assertMatchAll(
      r4, { { "0.5.1-alpha3", "0.5.1-alpha4", "0.5.1-beta", "0.5.1", "0.5.5" } }
  );
  assertMatchNone(
      r4, { { "0.5.1-alpha1", "0.5.2-alpha3", "0.5.5-pre", "0.5.0-pre" } }
  );
  assertMatchNone(r4, { { "0.6.0", "0.6.0-pre" } });

  assertException<VersionReqError>(
      []() { VersionReq::parse(">1.2.3 - <2.3.4"); },
      "invalid version requirement:\n"
      ">1.2.3 - <2.3.4\n"
      "       ^ expected `&&`"
  );

  pass();
}

void
testPre() {
  const auto r = VersionReq::parse("=2.1.1-really.0");
  assertMatchAll(r, { { "2.1.1-really.0" } });

  pass();
}

void
testCanonicalizeNoOp() {
  // 1.1. `A.B.C` (where A > 0) is equivalent to `>=A.B.C && <(A+1).0.0`
  assertEq(
      VersionReq::parse("1.2.3").canonicalize().toString(), ">=1.2.3 && <2.0.0"
  );

  // 1.2. `A.B` (where A > 0 & B > 0) is equivalent to `^A.B.0` (i.e., 1.1)
  assertEq(
      VersionReq::parse("1.2").canonicalize().toString(), ">=1.2.0 && <2.0.0"
  );

  // 1.3. `A` is equivalent to `=A` (i.e., 2.3)
  assertEq(
      VersionReq::parse("1").canonicalize().toString(), ">=1.0.0 && <2.0.0"
  );

  // 1.4. `0.B.C` (where B > 0) is equivalent to `>=0.B.C && <0.(B+1).0`
  assertEq(
      VersionReq::parse("0.2.3").canonicalize().toString(), ">=0.2.3 && <0.3.0"
  );

  // 1.5. `0.0.C` is equivalent to `=0.0.C` (i.e., 2.1)
  assertEq(VersionReq::parse("0.0.3").canonicalize().toString(), "=0.0.3");

  // 1.6. `0.0` is equivalent to `=0.0` (i.e., 2.2)
  assertEq(
      VersionReq::parse("0.0").canonicalize().toString(), ">=0.0.0 && <0.1.0"
  );

  pass();
}

void
testCanonicalizeExact() {
  // 2.1. `=A.B.C` is exactly the version `A.B.C`
  assertEq(VersionReq::parse("=1.2.3").canonicalize().toString(), "=1.2.3");

  // 2.2. `=A.B` is equivalent to `>=A.B.0 && <A.(B+1).0`
  assertEq(
      VersionReq::parse("=1.2").canonicalize().toString(), ">=1.2.0 && <1.3.0"
  );

  // 2.3. `=A` is equivalent to `>=A.0.0 && <(A+1).0.0`
  assertEq(
      VersionReq::parse("=1").canonicalize().toString(), ">=1.0.0 && <2.0.0"
  );

  pass();
}

void
testCanonicalizeGt() {
  // 3.1. `>A.B.C` is equivalent to `>=A.B.(C+1)`
  assertEq(VersionReq::parse(">1.2.3").canonicalize().toString(), ">=1.2.4");

  // 3.2. `>A.B` is equivalent to `>=A.(B+1).0`
  assertEq(VersionReq::parse(">1.2").canonicalize().toString(), ">=1.3.0");

  // 3.3. `>A` is equivalent to `>=(A+1).0.0`
  assertEq(VersionReq::parse(">1").canonicalize().toString(), ">=2.0.0");

  pass();
}

void
testCanonicalizeGte() {
  // 4.1. `>=A.B.C`
  assertEq(VersionReq::parse(">=1.2.3").canonicalize().toString(), ">=1.2.3");

  // 4.2. `>=A.B` is equivalent to `>=A.B.0`
  assertEq(VersionReq::parse(">=1.2").canonicalize().toString(), ">=1.2.0");

  // 4.3. `>=A` is equivalent to `>=A.0.0`
  assertEq(VersionReq::parse(">=1").canonicalize().toString(), ">=1.0.0");

  pass();
}

void
testCanonicalizeLt() {
  // 5.1. `<A.B.C`
  assertEq(VersionReq::parse("<1.2.3").canonicalize().toString(), "<1.2.3");

  // 5.2. `<A.B` is equivalent to `<A.B.0`
  assertEq(VersionReq::parse("<1.2").canonicalize().toString(), "<1.2.0");

  // 5.3. `<A` is equivalent to `<A.0.0`
  assertEq(VersionReq::parse("<1").canonicalize().toString(), "<1.0.0");

  pass();
}

void
testCanonicalizeLte() {
  // 6.1. `<=A.B.C` is equivalent to `<A.B.(C+1)`
  assertEq(VersionReq::parse("<=1.2.3").canonicalize().toString(), "<1.2.4");

  // 6.2. `<=A.B` is equivalent to `<A.(B+1).0`
  assertEq(VersionReq::parse("<=1.2").canonicalize().toString(), "<1.3.0");

  // 6.3. `<=A` is equivalent to `<(A+1).0.0`
  assertEq(VersionReq::parse("<=1").canonicalize().toString(), "<2.0.0");

  pass();
}

void
testParse() {
  assertException<VersionReqError>(
      []() { VersionReq::parse("\0"); },
      "invalid version requirement:\n"
      "\n"
      "^ expected =, >=, <=, >, <, or version"
  );

  assertException<ComparatorError>(
      []() { VersionReq::parse(">= >= 0.0.2"); },
      "invalid comparator:\n"
      ">= >= 0.0.2\n"
      "     ^ expected version"
  );

  assertException<ComparatorError>(
      []() { VersionReq::parse(">== 0.0.2"); },
      "invalid comparator:\n"
      ">== 0.0.2\n"
      "   ^ expected version"
  );

  assertException<VersionReqError>(
      []() { VersionReq::parse("a.0.0"); },
      "invalid version requirement:\n"
      "a.0.0\n"
      "^ expected =, >=, <=, >, <, or version"
  );

  assertException<SemverError>(
      []() { VersionReq::parse("1.0.0-"); },
      "invalid semver:\n"
      "1.0.0-\n"
      "      ^ expected number or identifier"
  );

  assertException<ComparatorError>(
      []() { VersionReq::parse(">="); },
      "invalid comparator:\n"
      ">=\n"
      "  ^ expected version"
  );

  pass();
}

void
testComparatorParse() {
  assertException<SemverError>(
      []() { Comparator::parse("1.2.3-01"); },
      "invalid semver:\n"
      "1.2.3-01\n"
      "      ^ invalid leading zero"
  );

  assertException<SemverError>(
      []() { Comparator::parse("1.2.3+4."); },
      "invalid semver:\n"
      "1.2.3+4.\n"
      "        ^ expected identifier"
  );

  assertException<ComparatorError>(
      []() { Comparator::parse(">"); },
      "invalid comparator:\n"
      ">\n"
      " ^ expected version"
  );

  assertException<SemverError>(
      []() { Comparator::parse("1."); },
      "invalid semver:\n"
      "1.\n"
      "  ^ expected number"
  );

  assertException<SemverError>(
      []() { Comparator::parse("1.*."); },
      "invalid semver:\n"
      "1.*.\n"
      "  ^ expected number"
  );

  pass();
}

void
testLeadingDigitInPreAndBuild() {
  for (const auto& cmp : { "", "<", "<=", ">", ">=" }) {
    // digit then alpha
    assertNoException([&cmp]() { VersionReq::parse(cmp + "1.2.3-1a"s); });
    assertNoException([&cmp]() { VersionReq::parse(cmp + "1.2.3+1a"s); });

    // digit then alpha (leading zero)
    assertNoException([&cmp]() { VersionReq::parse(cmp + "1.2.3-01a"s); });
    assertNoException([&cmp]() { VersionReq::parse(cmp + "1.2.3+01"s); });

    // multiple
    assertNoException([&cmp]() { VersionReq::parse(cmp + "1.2.3-1+1"s); });
    assertNoException([&cmp]() { VersionReq::parse(cmp + "1.2.3-1-1+1-1-1"s); }
    );
    assertNoException([&cmp]() { VersionReq::parse(cmp + "1.2.3-1a+1a"s); });
    assertNoException([&cmp]() {
      VersionReq::parse(cmp + "1.2.3-1a-1a+1a-1a-1a"s);
    });
  }

  pass();
}

void
testValidSpaces() {
  assertNoException([]() { VersionReq::parse("   1.2    "); });
  assertNoException([]() { VersionReq::parse(">   1.2.3    "); });
  assertNoException([]() { VersionReq::parse("  <1.2.3 &&>= 1.2.3"); });
  assertNoException([]() {
    VersionReq::parse("  <  1.2.3  &&   >=   1.2.3   ");
  });
  assertNoException([]() { VersionReq::parse(" <1.2.3     &&   >1    "); });
  assertNoException([]() { VersionReq::parse("<1.2.3&& >=1.2.3"); });
  assertNoException([]() { VersionReq::parse("<1.2.3  &&>=1.2.3"); });
  assertNoException([]() { VersionReq::parse("<1.2.3&&>=1.2.3"); });

  pass();
}

void
testInvalidSpaces() {
  assertException<ComparatorError>(
      []() { VersionReq::parse(" <  =   1.2.3"); },
      "invalid comparator:\n"
      " <  =   1.2.3\n"
      "     ^ expected version"
  );
  assertException<VersionReqError>(
      []() { VersionReq::parse("<1.2.3 & & >=1.2.3"); },
      "invalid version requirement:\n"
      "<1.2.3 & & >=1.2.3\n"
      "       ^ expected `&&`"
  );

  pass();
}

void
testInvalidConjunction() {
  assertException<VersionReqError>(
      []() { VersionReq::parse("<1.2.3 &&"); },
      "invalid version requirement:\n"
      "<1.2.3 &&\n"
      "         ^ expected >=, <=, >, or <"
  );
  assertException<VersionReqError>(
      []() { VersionReq::parse("<1.2.3  <1.2.3"); },
      "invalid version requirement:\n"
      "<1.2.3  <1.2.3\n"
      "              ^ expected `&&`"
  );
  assertException<VersionReqError>(
      []() { VersionReq::parse("<1.2.3 && <1.2.3 &&"); },
      "invalid version requirement:\n"
      "<1.2.3 && <1.2.3 &&\n"
      "                 ^ expected end of string"
  );
  assertException<VersionReqError>(
      []() { VersionReq::parse("<1.2.3 && <1.2.3 && <1.2.3"); },
      "invalid version requirement:\n"
      "<1.2.3 && <1.2.3 && <1.2.3\n"
      "                 ^ expected end of string"
  );

  pass();
}

void
testNonComparatorChain() {
  assertException<VersionReqError>(
      []() { VersionReq::parse("1.2.3 && 4.5.6"); },
      "invalid version requirement:\n"
      "1.2.3 && 4.5.6\n"
      "      ^ NoOp and Exact cannot chain"
  );
  assertException<VersionReqError>(
      []() { VersionReq::parse("=1.2.3 && =4.5.6"); },
      "invalid version requirement:\n"
      "=1.2.3 && =4.5.6\n"
      "       ^ NoOp and Exact cannot chain"
  );
  assertException<VersionReqError>(
      []() { VersionReq::parse("1.2.3 && =4.5.6"); },
      "invalid version requirement:\n"
      "1.2.3 && =4.5.6\n"
      "      ^ NoOp and Exact cannot chain"
  );
  assertException<VersionReqError>(
      []() { VersionReq::parse("=1.2.3 && 4.5.6"); },
      "invalid version requirement:\n"
      "=1.2.3 && 4.5.6\n"
      "       ^ NoOp and Exact cannot chain"
  );

  assertException<VersionReqError>(
      []() { VersionReq::parse("<1.2.3 && 4.5.6"); },
      "invalid version requirement:\n"
      "<1.2.3 && 4.5.6\n"
      "          ^ expected >=, <=, >, or <"
  );
  assertException<VersionReqError>(
      []() { VersionReq::parse("<1.2.3 && =4.5.6"); },
      "invalid version requirement:\n"
      "<1.2.3 && =4.5.6\n"
      "          ^ expected >=, <=, >, or <"
  );

  pass();
}

void
testToString() {
  assertEq(
      VersionReq::parse("  <1.2.3  &&>=1.0 ").toString(), "<1.2.3 && >=1.0"
  );

  pass();
}

void
testToPkgConfigString() {
  assertEq(
      VersionReq::parse("  <1.2.3  &&>=1.0 ").toPkgConfigString("foo"),
      "foo < 1.2.3, foo >= 1.0.0"
  );

  assertEq(
      VersionReq::parse("1.2.3").toPkgConfigString("foo"),
      "foo >= 1.2.3, foo < 2.0.0"
  );

  assertEq(
      VersionReq::parse(">1.2.3").toPkgConfigString("foo"), "foo >= 1.2.4"
  );

  assertEq(VersionReq::parse("=1.2.3").toPkgConfigString("foo"), "foo = 1.2.3");

  assertEq(
      VersionReq::parse("=1.2").toPkgConfigString("foo"),
      "foo >= 1.2.0, foo < 1.3.0"
  );

  assertEq(VersionReq::parse("0.0.1").toPkgConfigString("foo"), "foo = 0.0.1");

  pass();
}

void
testCanSimplify() {
  assertFalse(VersionReq::parse("1.2.3").canSimplify());
  assertFalse(VersionReq::parse("=1.2.3").canSimplify());

  assertTrue(VersionReq::parse(">1 && >2").canSimplify());
  assertTrue(VersionReq::parse(">1 && >=2").canSimplify());
  assertTrue(VersionReq::parse(">=1 && >2").canSimplify());
  assertTrue(VersionReq::parse(">=1 && >=2").canSimplify());

  assertTrue(VersionReq::parse("<1 && <2").canSimplify());
  assertTrue(VersionReq::parse("<1 && <=2").canSimplify());
  assertTrue(VersionReq::parse("<=1 && <2").canSimplify());
  assertTrue(VersionReq::parse("<=1 && <=2").canSimplify());

  // TODO: 1 and 1 are the same, but we have to handle 1.0 and 1 as the same.
  // Currently, there is no way to do this.
  assertFalse(VersionReq::parse(">=1 && <=1").canSimplify());
  assertFalse(VersionReq::parse(">=1.0 && <=1").canSimplify());
  assertFalse(VersionReq::parse(">=1.0.0 && <=1").canSimplify());

  assertFalse(VersionReq::parse("<=1 && >=1").canSimplify());
  assertFalse(VersionReq::parse("<=1.0 && >=1").canSimplify());
  assertFalse(VersionReq::parse("<=1.0.0 && >=1").canSimplify());

  assertFalse(VersionReq::parse(">1 && <1").canSimplify());
  assertFalse(VersionReq::parse("<1 && >1").canSimplify());

  pass();
}

} // namespace tests

int
main() {
  tests::testBasic();
  tests::testExact();
  tests::testGreaterThan();
  tests::testLessThan();
  tests::testNoOp();
  tests::testMultiple();
  tests::testPre();
  tests::testParse();
  tests::testCanonicalizeNoOp();
  tests::testCanonicalizeExact();
  tests::testCanonicalizeGt();
  tests::testCanonicalizeGte();
  tests::testCanonicalizeLt();
  tests::testCanonicalizeLte();
  tests::testComparatorParse();
  tests::testLeadingDigitInPreAndBuild();
  tests::testValidSpaces();
  tests::testInvalidSpaces();
  tests::testInvalidConjunction();
  tests::testNonComparatorChain();
  tests::testToString();
  tests::testToPkgConfigString();
  tests::testCanSimplify();
}

#endif
