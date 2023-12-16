module;

// std
#include <cstddef>
#include <cstdint>
#include <optional>
#include <string>
#include <string_view>
#include <vector>

export module semver.parser;

import semver.exception;
import semver.lexer;
import semver.range;
import semver.token;

export namespace semver {

struct Parser {
  using StringType = std::string_view;
  using ValueType = StringType::value_type;

  Lexer lexer;
  Token c1;

  /// Construct a new parser for the given input.
  explicit Parser(StringType str) : lexer(str), c1(lexer.next()) {}

  /// Pop one token.
  inline auto pop() -> Token {
    Token c1_ = this->c1; // NOLINT(readability-identifier-naming)
    this->c1 = lexer.next();
    return c1_;
  }

  /// Peek one token.
  [[nodiscard]] inline auto peek() const noexcept -> Token {
    return this->c1;
  }

  /// Skip whitespace if present.
  void skip_whitespace() {
    if (peek().is_whitespace()) {
      pop();
    }
  }

  /// Parse an optional comma separator, then if that is present a predicate.
  auto comma_predicate() -> std::optional<Predicate> {
    const bool has_comma = has_ws_separator(Token::Comma);

    if (auto predicate = this->predicate()) {
      return predicate;
    } else if (has_comma) {
      return std::nullopt; // Err(EmptyPredicate)
    } else {
      return std::nullopt;
    }
  }

  /// Parse an optional or separator `||`, then if that is present a range.
  auto or_range() -> std::optional<VersionReq> {
    if (!this->has_ws_separator(Token::Or)) {
      return std::nullopt;
    }
    return this->range();
  }

  /// Parse a single component.
  ///
  /// Returns `None` if the component is a wildcard.
  auto component() -> std::optional<std::uint_fast64_t> {
    if (const Token token = this->pop(); token.kind == Token::Numeric) {
      return std::get<Token::NumericType>(token.component);
    } else if (token.is_wildcard()) {
      return std::nullopt;
    } else {
      return std::nullopt; // Err(UnexpectedToken(tok))
    }
  }

  /// Parse a single numeric.
  auto numeric() -> std::optional<std::uint_fast64_t> {
    if (const Token token = this->pop(); token.kind == Token::Numeric) {
      return std::get<Token::NumericType>(token.component);
    }
    return std::nullopt;
  }

  /// Optionally parse a dot, then a component.
  ///
  /// The second component of the tuple indicates if a wildcard has been
  /// encountered, and is always `false` if the first component is `Some`.
  ///
  /// If a dot is not encountered, `(None, false)` is returned.
  ///
  /// If a wildcard is encountered, `(None, true)` is returned.
  auto dot_component() -> std::optional<std::uint_fast64_t> {
    if (this->peek() != Token::Dot) {
      return std::nullopt;
    }
    // pop the peeked dot.
    this->pop();
    return this->component();
  }

  /// Parse a dot, then a numeric.
  auto dot_numeric() -> std::optional<std::uint_fast64_t> {
    if (pop() != Token::Dot) {
      return std::nullopt;
    }
    return numeric();
  }

  /// Parse an string identifier.
  ///
  /// Like, `foo`, or `bar`.
  auto identifier() -> std::optional<Identifier> {
    const Token& token = pop();
    if (token.kind == Token::AlphaNumeric) {
      return Identifier(
          Identifier::AlphaNumeric,
          std::get<Token::AlphanumericType>(token.component)
      );
    } else if (token.kind == Token::Numeric) {
      return Identifier(
          Identifier::Numeric, std::get<Token::NumericType>(token.component)
      );
    }
    return std::nullopt;
  }

  /// Parse all pre-release identifiers, separated by dots.
  ///
  /// Like, `abcdef.1234`.
  auto pre() -> std::vector<Identifier> {
    if (const Token p = peek(); p.kind == Token::Whitespace) {
      pop(); // Drop whitespace
      if (const Token p2 = peek();
          p2 != Token::Unexpected && !p2.is_whitespace()) {
        // `1.2.3 a.b.c`
        throw VersionError(
            "continuing pre-release identifiers after spaces is not allowed"
        );
      }
      return {};
    } else if (p != Token::Hyphen) {
      return {};
    }
    // pop the peeked hyphen.
    pop();
    return parts();
  }

  /// Parse a dot-separated set of identifiers.
  auto parts() -> std::vector<Identifier> {
    std::vector<Identifier> parts{};
    parts.push_back(identifier().value());

    while (peek() == Token::Dot) {
      // pop the peeked hyphen.
      pop();
      parts.push_back(identifier().value());
    }
    return parts;
  }

  /// Parse optional build metadata.
  ///
  /// Like, `` (empty), or `+abcdef`.
  auto plus_build_metadata() -> std::vector<Identifier> {
    if (peek() != Token::Plus) {
      return {};
    }
    // pop the peeked plus.
    pop();
    return parts();
  }

  /// Optionally parse a single operator.
  ///
  /// Like, `~`, or `^`.
  auto op() -> Op {
    Op op(Op::Compatible);

    switch (peek().kind) {
      case Token::Eq:
        op.kind = Op::Ex;
        break;
      case Token::Gt:
        op.kind = Op::Gt;
        break;
      case Token::GtEq:
        op.kind = Op::GtEq;
        break;
      case Token::Lt:
        op.kind = Op::Lt;
        break;
      case Token::LtEq:
        op.kind = Op::LtEq;
        break;
      case Token::Tilde:
        op.kind = Op::Tilde;
        break;
      case Token::Caret:
        op.kind = Op::Compatible;
        break;
      // default op
      default:
        op.kind = Op::Compatible;
        break;
    }
    // remove the matched token.
    pop();
    skip_whitespace();
    return op;
  }

  /// Parse a single predicate.
  ///
  /// Like, `^1`, or `>=2.0.0`.
  auto predicate() -> std::optional<Predicate> {
    if (is_eof()) {
      return std::nullopt;
    }

    Op op = this->op();

    std::uint_fast64_t major;
    if (const auto m = this->component()) {
      major = m.value();
    } else {
      return std::nullopt;
    }

    const auto minor = this->dot_component();
    const auto patch = this->dot_component();
    const auto pre = this->pre();

    // TODO(ken-matsui): avoid illegal combinations, like `1.*.0`.
    if (!minor.has_value()) {
      op = Op(Op::Wildcard, WildcardVersion::Minor);
    }
    if (!patch.has_value()) {
      op = Op(Op::Wildcard, WildcardVersion::Patch);
    }

    // ignore build metadata
    this->plus_build_metadata();

    return Predicate{op, major, minor, patch, pre};
  }

  /// Parse a single range.
  ///
  /// Like, `^1.0` or `>=3.0.0, <4.0.0`.
  auto range() -> VersionReq {
    std::vector<Predicate> predicates{};

    if (const auto predicate = this->predicate()) {
      predicates.push_back(predicate.value());

      while (const auto next = this->comma_predicate()) {
        predicates.push_back(next.value());
      }
    }
    return VersionReq{predicates};
  }

  /// Parse a comparator.
  ///
  /// Like, `1.0 || 2.0` or `^1 || >=3.0.0, <4.0.0`.
  auto comparator() -> Comparator {
    std::vector<VersionReq> ranges{};
    ranges.push_back(this->range());

    while (const auto next = this->or_range()) {
      ranges.push_back(next.value());
    }
    return Comparator{ranges};
  }

  /// Parse a version.
  ///
  /// Like, `1.0.0` or `3.0.0-beta.1`.
  auto version() -> Version {
    this->skip_whitespace();

    const std::uint_fast64_t major = this->numeric().value();
    const std::uint_fast64_t minor = this->dot_numeric().value();
    const std::uint_fast64_t patch = this->dot_numeric().value();
    const std::vector<Identifier> pre = this->pre();
    const std::vector<Identifier> build = this->plus_build_metadata();

    this->skip_whitespace();

    return Version{major, minor, patch, pre, build};
  }

  /// Check if we have reached the end of input.
  [[nodiscard]] inline auto is_eof() const -> bool {
    return lexer.size() < lexer.c1_index;
  }

  /// Get the rest of the tokens in the parser.
  ///
  /// Useful for debugging.
  auto tail() -> std::vector<Token> {
    std::vector<Token> out{};

    out.push_back(c1);
    for (const Token token = lexer.next(); token != Token::Unexpected;) {
      out.push_back(token);
    }
    return out;
  }

private:
  auto has_ws_separator(const Token::Kind& pat) -> bool {
    skip_whitespace();

    if (peek() == pat) {
      // pop the separator.
      pop();
      // strip suffixing whitespace.
      skip_whitespace();
      return true;
    }
    return false;
  }
};

auto parse(std::string_view input) -> Version {
  try {
    Parser parser(input);
    return parser.version();
  } catch (const std::bad_optional_access&) {
    throw semver::VersionError("invalid version found: " + std::string(input));
  }
}

//  namespace range {
//      VersionReq
//      parse(std::string_view input) {
//      }
//  } // end namespace range

} // end namespace semver
