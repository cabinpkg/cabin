//! Typed model for `cfg(...)` target-condition expressions.
//!
//! Cabin manifests can declare *target-specific* dependency
//! tables, e.g.
//!
//! ```toml
//! [target.'cfg(os = "linux")'.dependencies]
//! epoll = "^1"
//! ```
//!
//! The condition string between the parentheses is parsed into a
//! [`Condition`] AST and evaluated against a [`TargetPlatform`]
//! describing the current evaluation context (the host build
//! platform in this step). Parsing and evaluation are pure,
//! deterministic, and side-effect-free.
//!
//! Supported keys are intentionally narrow — `os`, `arch`,
//! `family`, `env`, `abi`, `target` — and listed by the
//! [`ConditionKey`] enum. Any other key is rejected at parse
//! time so manifests do not silently rely on a future
//! detection layer.
//!
//! Public syntax is preserved as the canonical inner-expression
//! string when round-tripped (see the `Display` impl on
//! [`Condition`]); the manifest layer wraps it in `cfg(...)` and
//! the metadata layer emits the bare inner form so JSON /
//! on-disk shapes stay compact.

use std::collections::BTreeSet;
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error;

/// Typed AST for a `cfg(...)` target condition.
///
/// The wire format matches the manifest text: a key/value
/// (`key = "value"`) leaf, or one of the `all` / `any` / `not`
/// combinators. Equality and ordering are structural, so
/// identical expressions always compare equal regardless of
/// whitespace or quote style in the original source.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Condition {
    /// `key = "value"`. The key is restricted to the
    /// [`ConditionKey`] set; the value is a free-form ASCII
    /// string interpreted by [`evaluate`](Self::evaluate).
    KeyValue { key: ConditionKey, value: String },
    /// `feature = "name"`. Evaluates against the *enabled-feature
    /// set of the package the condition belongs to*, not the
    /// platform. Feature conditions are only meaningful — and only
    /// accepted — in flag tables (`[target.'cfg(...)'.profile]`);
    /// the manifest layer rejects a feature-referencing `cfg` that
    /// gates a dependency table, because feature resolution itself
    /// runs over the dependency graph and a feature→dependency edge
    /// would be circular.
    Feature(String),
    /// `all(<conditions>)`. Empty `all()` is rejected at parse
    /// time.
    All(Vec<Condition>),
    /// `any(<conditions>)`. Empty `any()` is rejected at parse
    /// time.
    Any(Vec<Condition>),
    /// `not(<single condition>)`.
    Not(Box<Condition>),
}

impl Condition {
    /// Parse a full `cfg(...)` expression. The wrapping
    /// `cfg(...)` is required so the parser is symmetric with
    /// the manifest text users write.
    ///
    /// # Errors
    /// Returns [`ConditionParseError::ExpectedCfgPrefix`] when the input is not
    /// wrapped in `cfg(`, [`ConditionParseError::UnbalancedParens`] when the
    /// trailing `)` is missing, and propagates any [`ConditionParseError`] from
    /// parsing the inner expression.
    pub fn parse_cfg(input: &str) -> Result<Self, ConditionParseError> {
        let trimmed = input.trim();
        let inner = trimmed
            .strip_prefix("cfg")
            .ok_or_else(|| ConditionParseError::ExpectedCfgPrefix(trimmed.to_owned()))?
            .trim_start();
        let inner = inner
            .strip_prefix('(')
            .ok_or_else(|| ConditionParseError::ExpectedCfgPrefix(trimmed.to_owned()))?;
        let inner = inner
            .strip_suffix(')')
            .ok_or_else(|| ConditionParseError::UnbalancedParens(trimmed.to_owned()))?;
        Self::parse_inner(inner)
    }

    /// Parse the inner expression of a `cfg(...)` form (no
    /// `cfg(` prefix or trailing `)`). Useful for the metadata
    /// round-trip path, where we store the inner form.
    ///
    /// # Errors
    /// Returns a [`ConditionParseError`] when the expression is malformed —
    /// e.g. an unsupported key, a missing `=` or quoted value, an empty
    /// `all()`/`any()`, a `not()` of wrong arity, unbalanced parentheses, or
    /// trailing input after the expression.
    pub fn parse_inner(input: &str) -> Result<Self, ConditionParseError> {
        let mut parser = Parser::new(input);
        let cond = parser.parse_condition()?;
        parser.expect_eof()?;
        Ok(cond)
    }

    /// Evaluate this condition against the typed
    /// [`ConditionContext`] — the host platform, the set of
    /// features enabled on the owning package, and the detected
    /// compiler identities. The result is fully determined by
    /// those inputs and the condition's AST — no global state, no
    /// environment lookup, no I/O.
    ///
    /// Contexts that carry no feature information (every
    /// dependency-gating call) use
    /// [`ConditionContext::platform_only`]; this is
    /// correct-by-construction because a feature-referencing `cfg`
    /// is rejected on dependency tables at manifest-load time, so a
    /// `Feature` leaf can only be reached here through a flag table
    /// that threaded the real enabled-feature set in.
    pub fn evaluate(&self, ctx: &ConditionContext<'_>) -> bool {
        match self {
            Condition::KeyValue { key, value } => key.lookup(ctx.platform) == value,
            Condition::Feature(name) => ctx.features.contains(name),
            Condition::All(items) => items.iter().all(|c| c.evaluate(ctx)),
            Condition::Any(items) => items.iter().any(|c| c.evaluate(ctx)),
            Condition::Not(inner) => !inner.evaluate(ctx),
        }
    }

    /// Whether this condition references any `feature = "..."`
    /// leaf. Used by the manifest layer to reject feature
    /// conditions on dependency tables (where they would be
    /// circular) while allowing them on flag tables.
    pub fn references_feature(&self) -> bool {
        match self {
            Condition::Feature(_) => true,
            Condition::KeyValue { .. } => false,
            Condition::All(items) | Condition::Any(items) => {
                items.iter().any(Condition::references_feature)
            }
            Condition::Not(inner) => inner.references_feature(),
        }
    }
}

impl fmt::Display for Condition {
    /// Canonical string form. Round-trips through
    /// [`Condition::parse_inner`].
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Condition::KeyValue { key, value } => write!(f, "{} = \"{}\"", key.as_str(), value),
            Condition::Feature(name) => write!(f, "feature = \"{name}\""),
            Condition::All(items) => {
                f.write_str("all(")?;
                write_list(f, items)?;
                f.write_str(")")
            }
            Condition::Any(items) => {
                f.write_str("any(")?;
                write_list(f, items)?;
                f.write_str(")")
            }
            Condition::Not(inner) => write!(f, "not({inner})"),
        }
    }
}

fn write_list(f: &mut fmt::Formatter<'_>, items: &[Condition]) -> fmt::Result {
    for (i, c) in items.iter().enumerate() {
        if i > 0 {
            f.write_str(", ")?;
        }
        write!(f, "{c}")?;
    }
    Ok(())
}

/// Recognized target-condition keys. Anything else is rejected
/// at parse time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ConditionKey {
    /// Operating system name (`linux`, `macos`, `windows`, …).
    Os,
    /// CPU architecture (`x86_64`, `aarch64`, …).
    Arch,
    /// OS family (`unix`, `windows`, …).
    Family,
    /// Toolchain environment (`gnu`, `musl`, `msvc`, …).
    Env,
    /// Application binary interface flavor (`eabi`, …).
    Abi,
    /// Full normalized target triple, when available.
    Target,
}

impl ConditionKey {
    pub const fn as_str(self) -> &'static str {
        match self {
            ConditionKey::Os => "os",
            ConditionKey::Arch => "arch",
            ConditionKey::Family => "family",
            ConditionKey::Env => "env",
            ConditionKey::Abi => "abi",
            ConditionKey::Target => "target",
        }
    }

    /// All recognized keys, in canonical declaration order.
    pub const fn all() -> &'static [ConditionKey] {
        &[
            ConditionKey::Os,
            ConditionKey::Arch,
            ConditionKey::Family,
            ConditionKey::Env,
            ConditionKey::Abi,
            ConditionKey::Target,
        ]
    }

    fn lookup(self, platform: &TargetPlatform) -> &str {
        match self {
            ConditionKey::Os => platform.os.as_str(),
            ConditionKey::Arch => platform.arch.as_str(),
            ConditionKey::Family => platform.family.as_str(),
            ConditionKey::Env => platform.env.as_str(),
            ConditionKey::Abi => platform.abi.as_str(),
            ConditionKey::Target => platform.target.as_str(),
        }
    }
}

impl FromStr for ConditionKey {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "os" => Ok(ConditionKey::Os),
            "arch" => Ok(ConditionKey::Arch),
            "family" => Ok(ConditionKey::Family),
            "env" => Ok(ConditionKey::Env),
            "abi" => Ok(ConditionKey::Abi),
            "target" => Ok(ConditionKey::Target),
            _ => Err(()),
        }
    }
}

/// Evaluation context for [`Condition::evaluate`]. Each field
/// is a stable, normalized lowercase string. Unknown values
/// flow through as the literal `unknown`, which is matchable in
/// `cfg(...)` expressions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TargetPlatform {
    pub os: String,
    pub arch: String,
    pub family: String,
    pub env: String,
    pub abi: String,
    pub target: String,
}

impl TargetPlatform {
    /// Best-effort detection of the *host* platform — the
    /// platform commands like `cabin build` execute on. Cabin
    /// does not yet support cross-compilation; future steps may
    /// add an explicit target-triple selection layer that wraps
    /// this constructor.
    pub fn current() -> Self {
        let os = normalize_os(std::env::consts::OS);
        let arch = normalize_arch(std::env::consts::ARCH);
        let family = normalize_family(std::env::consts::FAMILY, &os);
        let env = normalize_env(&os);
        let abi = "unknown".to_owned();
        let target = format!("{arch}-{family}-{os}");
        Self {
            os,
            arch,
            family,
            env,
            abi,
            target,
        }
    }
}

fn normalize_os(raw: &str) -> String {
    match raw {
        "linux" | "macos" | "windows" | "freebsd" | "openbsd" | "netbsd" | "dragonfly"
        | "android" | "ios" => raw.to_owned(),
        // Map common aliases.
        "darwin" => "macos".to_owned(),
        "" => "unknown".to_owned(),
        other => other.to_owned(),
    }
}

fn normalize_arch(raw: &str) -> String {
    match raw {
        "x86_64" | "aarch64" | "arm" | "riscv64" | "wasm32" => raw.to_owned(),
        "" => "unknown".to_owned(),
        other => other.to_owned(),
    }
}

fn normalize_family(raw: &str, os: &str) -> String {
    match raw {
        "unix" | "windows" | "wasm" => raw.to_owned(),
        _ => match os {
            "linux" | "macos" | "freebsd" | "openbsd" | "netbsd" | "dragonfly" | "android"
            | "ios" => "unix".to_owned(),
            "windows" => "windows".to_owned(),
            _ => "unknown".to_owned(),
        },
    }
}

fn normalize_env(os: &str) -> String {
    // The host environment cannot be detected from the Rust
    // standard library alone. We map the obvious cases so users
    // can write `cfg(env = "gnu")` etc., and fall back to
    // `unknown` everywhere else so unsupported queries are
    // explicit rather than silently false.
    match os {
        "linux" => "gnu".to_owned(),
        "macos" | "ios" => "apple".to_owned(),
        "windows" => "msvc".to_owned(),
        _ => "unknown".to_owned(),
    }
}

/// Evaluation context for [`Condition::evaluate`]. Bundles the
/// host platform, the owning package's enabled-feature set, and
/// the detected compiler identities so each leaf kind reads the
/// input it is defined over. Platform-only call sites (dependency
/// gating, toolchain / wrapper selection) use
/// [`ConditionContext::platform_only`]; compiler identities are
/// attached only on the flag-resolution path, the only place
/// compiler-referencing leaves are reachable (the manifest layer
/// rejects them elsewhere).
#[derive(Debug, Clone, Copy)]
pub struct ConditionContext<'a> {
    pub platform: &'a TargetPlatform,
    pub features: &'a BTreeSet<String>,
    /// Detected C compiler identity, when detection has run and
    /// resolved a C compiler.
    pub cc: Option<&'a crate::compiler::CompilerIdentity>,
    /// Detected C++ compiler identity, when detection has run.
    pub cxx: Option<&'a crate::compiler::CompilerIdentity>,
}

static EMPTY_FEATURES: BTreeSet<String> = BTreeSet::new();

impl<'a> ConditionContext<'a> {
    /// Platform-only evaluation: no features, no detected
    /// compilers. Correct for dependency gating and toolchain /
    /// wrapper selection, where feature and compiler leaves are
    /// rejected at manifest-load time.
    pub fn platform_only(platform: &'a TargetPlatform) -> Self {
        Self {
            platform,
            features: &EMPTY_FEATURES,
            cc: None,
            cxx: None,
        }
    }

    /// Platform + enabled-feature evaluation (no detected
    /// compilers attached yet).
    pub fn with_features(platform: &'a TargetPlatform, features: &'a BTreeSet<String>) -> Self {
        Self {
            platform,
            features,
            cc: None,
            cxx: None,
        }
    }

    /// Attach detected compiler identities (flag-resolution path).
    #[must_use]
    pub fn with_compilers(
        mut self,
        cc: Option<&'a crate::compiler::CompilerIdentity>,
        cxx: Option<&'a crate::compiler::CompilerIdentity>,
    ) -> Self {
        self.cc = cc;
        self.cxx = cxx;
        self
    }
}

// ---------------------------------------------------------------
// Parser.
// ---------------------------------------------------------------

/// Errors produced while parsing a `cfg(...)` expression.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ConditionParseError {
    #[error("expected a `cfg(...)` expression but found {0:?}")]
    ExpectedCfgPrefix(String),

    #[error("`cfg(...)` expression has unbalanced parentheses: {0:?}")]
    UnbalancedParens(String),

    #[error(
        "unsupported target cfg key {key:?}; supported keys are os, arch, family, env, abi, and target"
    )]
    UnsupportedKey { key: String },

    #[error("expected `=` after key {key:?} in cfg expression")]
    ExpectedEquals { key: String },

    #[error("expected a quoted string value for key {key:?} in cfg expression; got {found:?}")]
    ExpectedQuotedValue { key: String, found: String },

    #[error("unterminated string literal in cfg expression: {0:?}")]
    UnterminatedString(String),

    #[error("trailing input after cfg expression: {0:?}")]
    TrailingInput(String),

    #[error("`all()` requires at least one condition")]
    EmptyAll,

    #[error("`any()` requires at least one condition")]
    EmptyAny,

    #[error("`not()` takes exactly one condition; found {0}")]
    NotArity(usize),

    #[error("expected `(` after {0}")]
    ExpectedOpenParen(&'static str),

    #[error("expected `)` to close {0}")]
    ExpectedCloseParen(&'static str),

    #[error("unexpected token in cfg expression: {0:?}")]
    UnexpectedToken(String),

    #[error("empty cfg expression")]
    Empty,
}

struct Parser<'a> {
    src: &'a str,
    pos: usize,
}

impl<'a> Parser<'a> {
    fn new(src: &'a str) -> Self {
        Self { src, pos: 0 }
    }

    fn skip_whitespace(&mut self) {
        while let Some(c) = self.peek_char() {
            if c.is_whitespace() {
                self.pos += c.len_utf8();
            } else {
                break;
            }
        }
    }

    fn peek_char(&self) -> Option<char> {
        self.src[self.pos..].chars().next()
    }

    fn expect_eof(&mut self) -> Result<(), ConditionParseError> {
        self.skip_whitespace();
        if self.pos < self.src.len() {
            Err(ConditionParseError::TrailingInput(
                self.src[self.pos..].to_owned(),
            ))
        } else {
            Ok(())
        }
    }

    fn parse_condition(&mut self) -> Result<Condition, ConditionParseError> {
        self.skip_whitespace();
        if self.pos >= self.src.len() {
            return Err(ConditionParseError::Empty);
        }
        // Read an identifier. It is either a combinator (`all`,
        // `any`, `not`) or a key in the recognized set.
        let ident = self.read_ident()?;
        self.skip_whitespace();
        match ident.as_str() {
            "all" => {
                self.expect_open_paren("all")?;
                let items = self.parse_condition_list()?;
                self.expect_close_paren("all")?;
                if items.is_empty() {
                    return Err(ConditionParseError::EmptyAll);
                }
                Ok(Condition::All(items))
            }
            "any" => {
                self.expect_open_paren("any")?;
                let items = self.parse_condition_list()?;
                self.expect_close_paren("any")?;
                if items.is_empty() {
                    return Err(ConditionParseError::EmptyAny);
                }
                Ok(Condition::Any(items))
            }
            "not" => {
                self.expect_open_paren("not")?;
                let items = self.parse_condition_list()?;
                self.expect_close_paren("not")?;
                if items.len() != 1 {
                    return Err(ConditionParseError::NotArity(items.len()));
                }
                let inner = items.into_iter().next().expect("len==1 above");
                Ok(Condition::Not(Box::new(inner)))
            }
            other => {
                // `feature` and the platform keys share the
                // `ident = "value"` shape; parse the `= "value"`
                // tail once, then dispatch on the identifier.
                self.skip_whitespace();
                if self.peek_char() != Some('=') {
                    return Err(ConditionParseError::ExpectedEquals {
                        key: other.to_owned(),
                    });
                }
                self.pos += 1; // consume '='
                self.skip_whitespace();
                let value = self.read_quoted_string(other)?;
                if other == "feature" {
                    Ok(Condition::Feature(value))
                } else {
                    let key = ConditionKey::from_str(other).map_err(|()| {
                        ConditionParseError::UnsupportedKey {
                            key: other.to_owned(),
                        }
                    })?;
                    Ok(Condition::KeyValue { key, value })
                }
            }
        }
    }

    fn parse_condition_list(&mut self) -> Result<Vec<Condition>, ConditionParseError> {
        let mut items = Vec::new();
        self.skip_whitespace();
        if self.peek_char() == Some(')') {
            return Ok(items);
        }
        loop {
            let cond = self.parse_condition()?;
            items.push(cond);
            self.skip_whitespace();
            match self.peek_char() {
                Some(',') => {
                    self.pos += 1;
                    self.skip_whitespace();
                }
                _ => break,
            }
        }
        Ok(items)
    }

    fn expect_open_paren(&mut self, what: &'static str) -> Result<(), ConditionParseError> {
        self.skip_whitespace();
        if self.peek_char() == Some('(') {
            self.pos += 1;
            Ok(())
        } else {
            Err(ConditionParseError::ExpectedOpenParen(what))
        }
    }

    fn expect_close_paren(&mut self, what: &'static str) -> Result<(), ConditionParseError> {
        self.skip_whitespace();
        if self.peek_char() == Some(')') {
            self.pos += 1;
            Ok(())
        } else {
            Err(ConditionParseError::ExpectedCloseParen(what))
        }
    }

    fn read_ident(&mut self) -> Result<String, ConditionParseError> {
        let start = self.pos;
        while let Some(c) = self.peek_char() {
            if c.is_ascii_alphanumeric() || c == '_' {
                self.pos += c.len_utf8();
            } else {
                break;
            }
        }
        if start == self.pos {
            return Err(ConditionParseError::UnexpectedToken(
                self.src[self.pos..].to_owned(),
            ));
        }
        Ok(self.src[start..self.pos].to_owned())
    }

    fn read_quoted_string(&mut self, key: &str) -> Result<String, ConditionParseError> {
        if self.peek_char() != Some('"') {
            // Capture the offending token (rest of input up to a
            // delimiter) so the error message can show what we
            // saw.
            let rest_start = self.pos;
            while let Some(c) = self.peek_char() {
                if c == ',' || c == ')' || c.is_whitespace() {
                    break;
                }
                self.pos += c.len_utf8();
            }
            return Err(ConditionParseError::ExpectedQuotedValue {
                key: key.to_owned(),
                found: self.src[rest_start..self.pos].to_owned(),
            });
        }
        self.pos += 1;
        let start = self.pos;
        while let Some(c) = self.peek_char() {
            if c == '"' {
                let value = self.src[start..self.pos].to_owned();
                self.pos += 1;
                return Ok(value);
            }
            self.pos += c.len_utf8();
        }
        Err(ConditionParseError::UnterminatedString(
            self.src[start..].to_owned(),
        ))
    }
}

// ---------------------------------------------------------------
// Serde — Condition serializes as its canonical inner-expression
// string form so on-disk metadata stays compact and stable.
// ---------------------------------------------------------------

impl Serialize for Condition {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for Condition {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(d)?;
        Condition::parse_inner(&raw).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn linux_x86_64() -> TargetPlatform {
        TargetPlatform {
            os: "linux".into(),
            arch: "x86_64".into(),
            family: "unix".into(),
            env: "gnu".into(),
            abi: "unknown".into(),
            target: "x86_64-unix-linux".into(),
        }
    }

    fn macos_aarch64() -> TargetPlatform {
        TargetPlatform {
            os: "macos".into(),
            arch: "aarch64".into(),
            family: "unix".into(),
            env: "apple".into(),
            abi: "unknown".into(),
            target: "aarch64-unix-macos".into(),
        }
    }

    #[test]
    fn parses_simple_key_value() {
        let cond = Condition::parse_cfg(r#"cfg(os = "linux")"#).unwrap();
        assert_eq!(
            cond,
            Condition::KeyValue {
                key: ConditionKey::Os,
                value: "linux".into()
            }
        );
    }

    #[test]
    fn parses_each_supported_key() {
        for (raw, key) in [
            (r#"cfg(os = "linux")"#, ConditionKey::Os),
            (r#"cfg(arch = "x86_64")"#, ConditionKey::Arch),
            (r#"cfg(family = "unix")"#, ConditionKey::Family),
            (r#"cfg(env = "gnu")"#, ConditionKey::Env),
            (r#"cfg(abi = "eabi")"#, ConditionKey::Abi),
            (
                r#"cfg(target = "x86_64-unknown-linux-gnu")"#,
                ConditionKey::Target,
            ),
        ] {
            let cond = Condition::parse_cfg(raw).unwrap();
            match cond {
                Condition::KeyValue { key: k, .. } => assert_eq!(k, key, "{raw}"),
                other => panic!("{raw}: expected key/value, got {other:?}"),
            }
        }
    }

    #[test]
    fn parses_all_any_not() {
        let all = Condition::parse_cfg(r#"cfg(all(os = "linux", arch = "x86_64"))"#).unwrap();
        let any = Condition::parse_cfg(r#"cfg(any(os = "macos", os = "linux"))"#).unwrap();
        let not = Condition::parse_cfg(r#"cfg(not(os = "windows"))"#).unwrap();
        assert!(matches!(all, Condition::All(ref v) if v.len() == 2));
        assert!(matches!(any, Condition::Any(ref v) if v.len() == 2));
        assert!(matches!(not, Condition::Not(_)));
    }

    #[test]
    fn rejects_unquoted_value() {
        let err = Condition::parse_cfg(r"cfg(os = linux)").unwrap_err();
        match err {
            ConditionParseError::ExpectedQuotedValue { key, .. } => assert_eq!(key, "os"),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn rejects_unsupported_key() {
        let err = Condition::parse_cfg(r#"cfg(compiler = "clang")"#).unwrap_err();
        match err {
            ConditionParseError::UnsupportedKey { key } => assert_eq!(key, "compiler"),
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[test]
    fn rejects_empty_all_and_any() {
        assert!(matches!(
            Condition::parse_cfg("cfg(all())").unwrap_err(),
            ConditionParseError::EmptyAll
        ));
        assert!(matches!(
            Condition::parse_cfg("cfg(any())").unwrap_err(),
            ConditionParseError::EmptyAny
        ));
    }

    #[test]
    fn rejects_not_with_arity_other_than_one() {
        let err = Condition::parse_cfg(r#"cfg(not(os = "linux", arch = "x86_64"))"#).unwrap_err();
        assert!(matches!(err, ConditionParseError::NotArity(2)));
    }

    #[test]
    fn rejects_missing_cfg_prefix() {
        assert!(matches!(
            Condition::parse_cfg(r#"os = "linux""#).unwrap_err(),
            ConditionParseError::ExpectedCfgPrefix(_)
        ));
    }

    #[test]
    fn rejects_unbalanced_parens() {
        assert!(matches!(
            Condition::parse_cfg("cfg(os = \"linux\"").unwrap_err(),
            ConditionParseError::UnbalancedParens(_)
        ));
    }

    #[test]
    fn evaluates_simple_key_value() {
        let linux = linux_x86_64();
        let macos = macos_aarch64();
        let cond = Condition::parse_cfg(r#"cfg(os = "linux")"#).unwrap();
        assert!(cond.evaluate(&ConditionContext::platform_only(&linux)));
        assert!(!cond.evaluate(&ConditionContext::platform_only(&macos)));
    }

    #[test]
    fn evaluates_all_any_not() {
        let linux = linux_x86_64();
        let macos = macos_aarch64();
        let all = Condition::parse_cfg(r#"cfg(all(os = "linux", arch = "x86_64"))"#).unwrap();
        let any = Condition::parse_cfg(r#"cfg(any(os = "macos", os = "linux"))"#).unwrap();
        let not = Condition::parse_cfg(r#"cfg(not(os = "windows"))"#).unwrap();
        assert!(all.evaluate(&ConditionContext::platform_only(&linux)));
        assert!(!all.evaluate(&ConditionContext::platform_only(&macos)));
        assert!(any.evaluate(&ConditionContext::platform_only(&linux)));
        assert!(any.evaluate(&ConditionContext::platform_only(&macos)));
        assert!(not.evaluate(&ConditionContext::platform_only(&linux)));
        assert!(not.evaluate(&ConditionContext::platform_only(&macos)));
    }

    #[test]
    fn parses_and_evaluates_feature_leaf() {
        let linux = linux_x86_64();
        let cond = Condition::parse_cfg(r#"cfg(feature = "simd")"#).unwrap();
        assert_eq!(cond, Condition::Feature("simd".to_owned()));
        assert!(cond.references_feature());
        let enabled: BTreeSet<String> = BTreeSet::from(["simd".to_owned()]);
        assert!(cond.evaluate(&ConditionContext::with_features(&linux, &enabled)));
        assert!(!cond.evaluate(&ConditionContext::platform_only(&linux)));
    }

    #[test]
    fn evaluates_feature_combined_with_platform() {
        let linux = linux_x86_64();
        let macos = macos_aarch64();
        let cond = Condition::parse_cfg(r#"cfg(all(feature = "simd", arch = "x86_64"))"#).unwrap();
        assert!(cond.references_feature());
        let enabled: BTreeSet<String> = BTreeSet::from(["simd".to_owned()]);
        // Both the feature and the platform must hold.
        assert!(cond.evaluate(&ConditionContext::with_features(&linux, &enabled)));
        assert!(!cond.evaluate(&ConditionContext::with_features(&macos, &enabled))); // wrong arch
        assert!(!cond.evaluate(&ConditionContext::platform_only(&linux))); // feature off
    }

    #[test]
    fn references_feature_is_false_for_platform_only_conditions() {
        for raw in [
            r#"cfg(os = "linux")"#,
            r#"cfg(all(os = "linux", arch = "x86_64"))"#,
            r#"cfg(not(os = "windows"))"#,
        ] {
            assert!(!Condition::parse_cfg(raw).unwrap().references_feature());
        }
    }

    #[test]
    fn display_round_trips_through_parse_inner() {
        for raw in [
            r#"os = "linux""#,
            r#"feature = "simd""#,
            r#"all(feature = "simd", arch = "x86_64")"#,
            r#"all(os = "linux", arch = "x86_64")"#,
            r#"any(os = "macos", os = "linux")"#,
            r#"not(os = "windows")"#,
            r#"all(any(os = "linux", os = "macos"), not(arch = "wasm32"))"#,
        ] {
            let cond = Condition::parse_inner(raw).unwrap();
            let rendered = cond.to_string();
            assert_eq!(rendered, raw, "round-trip should be byte-identical");
            let again = Condition::parse_inner(&rendered).unwrap();
            assert_eq!(cond, again);
        }
    }

    #[test]
    fn current_target_platform_is_internally_consistent() {
        let p = TargetPlatform::current();
        // Each field is non-empty and lowercase ASCII.
        for v in [&p.os, &p.arch, &p.family, &p.env, &p.abi, &p.target] {
            assert!(!v.is_empty());
            assert!(v.chars().all(|c| !c.is_ascii_uppercase()));
        }
    }

    #[test]
    fn deterministic_serialization_for_metadata_round_trip() {
        let cond = Condition::parse_cfg(
            r#"cfg(all(os = "linux", any(arch = "x86_64", arch = "aarch64")))"#,
        )
        .unwrap();
        let json = serde_json::to_string(&cond).unwrap();
        let parsed: Condition = serde_json::from_str(&json).unwrap();
        assert_eq!(cond, parsed);
    }
}
