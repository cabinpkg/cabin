#include <boost/ut.hpp>
#include <poac/util/semver/token.hpp>

auto main() -> int {
  using namespace std::literals::string_literals;
  using namespace boost::ut;

  using semver::Token;

  "test parser::Token"_test = [] {
    {
      constexpr Token TOKEN{Token::Eq};
      expect(constant<TOKEN == Token::Eq>);
      expect(constant<std::holds_alternative<std::monostate>(TOKEN.component)>);
    }
    {
      constexpr Token TOKEN{Token::Whitespace, 0, 2};
      expect(constant<TOKEN.kind == Token::Whitespace>);
      expect(constant<
             TOKEN.component == Token::VariantType{std::make_pair(0, 2)}>);
    }
    {
      constexpr Token TOKEN{Token::Numeric, 0};
      expect(constant<TOKEN.kind == Token::Numeric>);
      expect(constant<
             TOKEN.component == Token::VariantType{std::uint_fast64_t{0}}>);
    }
    {
      constexpr Token TOKEN{Token::AlphaNumeric, "beta.2"};
      expect(constant<TOKEN.kind == Token::AlphaNumeric>);
      expect(constant<TOKEN.component == Token::VariantType{"beta.2"}>);
    }

    expect(throws<std::invalid_argument>([] {
      Token(Token::AlphaNumeric, 0, 2);
    }));
    expect(throws<std::invalid_argument>([] { Token(Token::Whitespace, 0); }));
    expect(throws<std::invalid_argument>([] { Token(Token::Numeric, "beta.2"); }
    ));
  };

  "test token is whitespace"_test = [] {
    expect(constant<Token(Token::Whitespace, 0, 2).is_whitespace()>);
    expect(constant<!Token(Token::Gt).is_whitespace()>);
  };

  "test token is simple token"_test = [] {
    expect(constant<Token(Token::Eq).is_simple_token()>);
    expect(constant<Token(Token::Gt).is_simple_token()>);
    expect(constant<Token(Token::Lt).is_simple_token()>);
    expect(constant<Token(Token::LtEq).is_simple_token()>);
    expect(constant<Token(Token::GtEq).is_simple_token()>);
    expect(constant<Token(Token::Caret).is_simple_token()>);
    expect(constant<Token(Token::Tilde).is_simple_token()>);
    expect(constant<Token(Token::Star).is_simple_token()>);
    expect(constant<Token(Token::Dot).is_simple_token()>);
    expect(constant<Token(Token::Comma).is_simple_token()>);
    expect(constant<Token(Token::Hyphen).is_simple_token()>);
    expect(constant<Token(Token::Plus).is_simple_token()>);
    expect(constant<Token(Token::Or).is_simple_token()>);
    expect(constant<Token(Token::Unexpected).is_simple_token()>);
    expect(constant<!Token(Token::Numeric, 3).is_simple_token()>);
  };

  "test token is wildcard"_test = [] {
    expect(constant<Token(Token::Star).is_wildcard()>);
    expect(constant<Token(Token::AlphaNumeric, "x").is_wildcard()>);
    expect(constant<Token(Token::AlphaNumeric, "X").is_wildcard()>);
    expect(constant<!Token(Token::AlphaNumeric, "other").is_wildcard()>);
  };

  "test token_eq"_test = [] {
    expect(constant<Token(Token::Star) == Token::Star>);
    expect(constant<Token::Star == Token(Token::Star)>);
    expect(constant<Token(Token::Star) == Token(Token::Star)>);
    expect(constant<
           Token(Token::Whitespace, 0, 2) == Token(Token::Whitespace, 0, 2)>);
    expect(constant<Token(Token::Numeric, 0) == Token(Token::Numeric, 0)>);
    expect(constant<
           Token(Token::AlphaNumeric, "x") == Token(Token::AlphaNumeric, "x")>);
  };

  "test token_neq"_test = [] {
    expect(constant<Token(Token::Star) != Token::Eq>);
    expect(constant<Token::Star != Token(Token::Eq)>);
    expect(constant<Token(Token::Eq) != Token(Token::Star)>);
    expect(constant<Token(Token::Whitespace, 0, 2) != Token::Whitespace>);
    expect(constant<Token::Whitespace != Token(Token::Whitespace, 0, 7)>);
    expect(constant<Token::Numeric != Token(Token::Numeric, 0)>);
    expect(constant<Token(Token::Numeric, 8) != Token(Token::Numeric, 0)>);
    expect(constant<
           Token(Token::AlphaNumeric, "x") != Token(Token::AlphaNumeric, "X")>);
    expect(constant<Token(Token::AlphaNumeric, "x") != Token::AlphaNumeric>);
    expect(constant<Token::Numeric != Token(Token::AlphaNumeric, "0")>);
    expect(constant<
           Token(Token::Numeric, 0) != Token(Token::AlphaNumeric, "0")>);
  };
}
