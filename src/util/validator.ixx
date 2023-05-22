module;

// std
#include <algorithm>
#include <iterator> // std::cbegin, std::cend

// external
#include <spdlog/spdlog.h> // NOLINT(build/include_order)
#include <toml.hpp>

// internal
#include "result-macros.hpp"

export module poac.util.validator;

import poac.config;
import poac.data.manifest;
import poac.util.format;
import poac.util.log;
import poac.util.result;
import poac.util.rustify;
import semver;

export namespace poac::util::validator {

[[nodiscard]] auto required_config_exists(bool find_parents = true)
    -> Result<Path, String> {
  // TODO(ken-matsui): move out to data/manifest.hpp
  Path candidate = config::cwd;
  do {
    std::error_code ec{};
    const Path config_path = candidate / data::manifest::NAME;
    spdlog::trace("Finding manifest: {} ...", config_path);
    if (fs::exists(config_path, ec)) {
      return Ok(config_path);
    }

    const Path parent_path = candidate.parent_path();
    if (candidate.has_parent_path()
        && parent_path != candidate.root_directory()) {
      candidate = parent_path;
    } else {
      break;
    }
  } while (find_parents);
  return Err(format(
      "could not find `{}` here{}", data::manifest::NAME,
      find_parents ? " and in its parents" : ""
  ));
}

[[nodiscard]] auto can_create_directory(const Path& p) -> Result<void, String> {
  std::error_code ec{}; // This is used for noexcept optimization

  const bool exists = fs::exists(p, ec);
  if (exists && !fs::is_directory(p, ec)) {
    return Err(format(
        "The `{}` directory could not be created "
        "because the same name file exists",
        p.string()
    ));
  } else if (exists && !fs::is_empty(p, ec)) {
    return Err(
        format("The `{}` directory already exists and is not empty", p.string())
    );
  }
  return Ok();
}

constexpr auto is_digit(const char c) noexcept -> bool {
  return '0' <= c && c <= '9';
}

constexpr auto is_alphabet(const char c) noexcept -> bool {
  return ('A' <= c && c <= 'Z') || ('a' <= c && c <= 'z');
}

constexpr auto is_alpha_numeric(const char c) noexcept -> bool {
  return is_digit(c) || is_alphabet(c);
}

[[nodiscard]] auto two_or_more_symbols(StringRef s) noexcept
    -> Result<void, String> {
  const usize slashes = std::count(s.begin(), s.end(), '/');
  if (slashes > 1) {
    return Err(
        "Invalid package name.\n"
        "It is prohibited to use two or more `/`."
    );
  }
  return Ok();
}

[[nodiscard]] auto start_with_symbol(StringRef s) noexcept
    -> Result<void, String> {
  if (s[0] == '_' || s[0] == '-' || s[0] == '/') {
    return Err(
        "Invalid package name.\n"
        "It is prohibited to use a string\n"
        " that starts with `_`, `-`, and `/`."
    );
  }
  return Ok();
}

[[nodiscard]] auto end_with_symbol(StringRef s) noexcept
    -> Result<void, String> {
  const char last = s[s.size() - 1];
  if (last == '_' || last == '-' || last == '/') {
    return Err(
        "Invalid package name.\n"
        "It is prohibited to use a string\n"
        " that ends with `_`, `-`, and `/`."
    );
  }
  return Ok();
}

[[nodiscard]] auto invalid_characters_impl(StringRef s) noexcept
    -> Result<void, String> {
  for (const char c : s) {
    if (!is_alpha_numeric(c) && c != '_' && c != '-' && c != '/') {
      return Err(
          "Invalid package name.\n"
          "It is prohibited to use a string\n"
          " that does not match ^([a-z|\\d|_|\\-|\\/]*)$."
      );
    }
  }
  return Ok();
}

[[nodiscard]] auto invalid_characters(StringRef s) noexcept
    -> Result<void, String> {
  Try(invalid_characters_impl(s));
  Try(start_with_symbol(s));
  Try(end_with_symbol(s));
  Try(two_or_more_symbols(s));
  return Ok();
}

[[nodiscard]] auto using_keywords(StringRef s) -> Result<void, String> {
  // Ban keywords
  // https://en.cppreference.com/w/cpp/keyword
  constexpr Arr<StringRef, 96> BLACKLIST{
      "alignas",
      "alignof",
      "and",
      "and_eq",
      "asm",
      "atomic_cancel",
      "atomic_commit",
      "atomic_noexcept",
      "auto",
      "bitand",
      "bitor",
      "bool",
      "break",
      "case",
      "catch",
      "char",
      "char8_t",
      "char16_t",
      "char32_t",
      "class",
      "compl",
      "concept",
      "const",
      "consteval",
      "constexpr",
      "const_cast",
      "continue",
      "co_await",
      "co_return",
      "co_yield",
      "decltype",
      "default",
      "delete",
      "do",
      "double",
      "dynamic_cast",
      "else",
      "enum",
      "explicit",
      "export",
      "extern",
      "false",
      "float",
      "for",
      "friend",
      "goto",
      "if",
      "inline",
      "int",
      "long",
      "mutable",
      "namespace",
      "new",
      "noexcept",
      "not",
      "not_eq",
      "nullptr",
      "operator",
      "or",
      "or_eq",
      "private",
      "protected",
      "public",
      "reflexpr",
      "register",
      "reinterpret_cast",
      "requires",
      "return",
      "short",
      "signed",
      "sizeof",
      "static",
      "static_assert",
      "static_cast",
      "struct",
      "switch",
      "synchronized",
      "template",
      "this",
      "thread_local",
      "throw",
      "true",
      "try",
      "typedef",
      "typeid",
      "typename",
      "union",
      "unsigned",
      "using",
      "virtual",
      "void",
      "volatile",
      "wchar_t",
      "while",
      "xor",
      "xor_eq",
  };
  if (std::find(std::cbegin(BLACKLIST), std::cend(BLACKLIST), s)
      != std::cend(BLACKLIST)) {
    return Err(
        format("`{}` is a keyword; it cannot be used as a package name", s)
    );
  }
  return Ok();
}

[[nodiscard]] auto one_char(StringRef s) -> Result<void, String> {
  if (s.empty()) {
    return Err(
        "Invalid package name.\n"
        "It is prohibited to use an empty string."
    );
  } else if (s.size() == 1) {
    return Err(
        "Invalid package name.\n"
        "It is prohibited to use one char."
    );
  }
  return Ok();
}

[[nodiscard]] auto valid_package_name(StringRef s) -> Result<void, String> {
  Try(one_char(s));
  Try(invalid_characters(s));
  Try(using_keywords(s));
  return Ok();
}

[[nodiscard]] auto valid_version(StringRef s) -> Result<void, String> {
  try {
    semver::parse(s);
  } catch (const semver::Exception& e) {
    return Err(format(
        "version `{}` is not compliant with the Semantic Versioning notation.\n"
        "For more information, please go to: https://semver.org/",
        String(s)
    ));
  }
  return Ok();
}

[[nodiscard]] auto valid_athr(StringRef s) -> Result<void, String> {
  // TODO(ken-matsui): Email address parser
  if (usize pos = s.find('<'); pos != None) {
    if (pos = s.find('@', pos + 1); pos != None) {
      if (pos = s.find('>', pos + 1); pos != None) {
        if (s.ends_with('>')) {
          return Ok();
        }
      }
    }
  }
  return Err(format(
      "author `{}` is written in invalid style. It should be like:\n"
      "  `Your Name <your-public@email.address>`",
      String(s)
  ));
}

[[nodiscard]] auto valid_authors(const Vec<String>& authors)
    -> Result<void, String> {
  if (authors.empty()) {
    return Err("key `authors` cannot be empty.");
  }
  for (const StringRef a : authors) {
    Try(valid_athr(a));
  }
  return Ok();
}

[[nodiscard]] auto valid_edition(const i32& edition) -> Result<void, String> {
  switch (edition) {
    case 1998:
    case 2003:
    case 2011:
    case 2014:
    case 2017:
    case 2020:
    case 2023:
      return Ok();
    default:
      return Err(format(
          "`{}` cannot be used as an `edition` key. Possible values are:\n"
          "  1998, 2003, 2011, 2014, 2017, 2020, and 2023",
          edition
      ));
  }
}

[[nodiscard]] auto valid_license(StringRef license) -> Result<void, String> {
  // This list is from https://choosealicense.com/licenses
  if (license == "AGPL-3.0" || license == "GPL-3.0" || license == "LGPL-3.0"
      || license == "MPL-2.0" || license == "Apache-2.0" || license == "MIT"
      || license == "BSL-1.0" || license == "Unlicense") {
    return Ok();
  }
  return Err(format(
      "`{}` cannot be used as `license` key. Possible values are:\n"
      "  `AGPL-3.0`, `GPL-3.0`, `LGPL-3.0`, `MPL-2.0`, `Apache-2.0`, "
      "`MIT`, `BSL-1.0`, and `Unlicense`",
      String(license)
  ));
}

[[nodiscard]] auto valid_repository(StringRef repo) -> Result<void, String> {
  // Can be parsed? it should be:
  // https://github.com/org/repo/tree/tag
  if (repo.starts_with("https://github.com/")) {
    // org/
    // 19: size of `https://github.com/`
    if (usize pos = repo.find('/', 19); pos != None) {
      // repo/
      if (pos = repo.find('/', pos + 1); pos != None) {
        // tree/
        if (repo.substr(pos + 1, 5) == "tree/") {
          // repo/tree/
          //     ^---->^
          pos += 5;
          // tag
          if (repo.size() - 1 > pos) {
            // -- tag found case
            // tree/tag `8 (size)`
            //      ^   `5 (pos)`

            // -- no tag case
            // tree/  `5 (size)`
            //      ^ `5 (pos)`
            return Ok();
          }
        }
      }
    }
  }
  return Err(format(
      "`{}` is invalid form for the `repository` key. It should be like:\n"
      "  `https://github.com/org/repo/tree/tag`",
      repo
  ));
}

[[nodiscard]] auto valid_description(StringRef desc) -> Result<void, String> {
  const usize size = desc.size();
  if (size < 10) {
    return Err(
        "the `description` key cannot be smaller than 10 characters long.\n"
        "The `description` must be descriptive."
    );
  } else if (size > 180) {
    return Err(
        "the `description` key cannot exceed 180 characters long.\n"
        "Please specify it more briefly."
    );
  }
  return Ok();
}

[[nodiscard]] auto valid_manifest(const toml::value& manifest)
    -> Result<data::manifest::PartialPackage, String> {
  const auto package =
      toml::find<data::manifest::PartialPackage>(manifest, "package");
  Try(valid_package_name(package.name));
  Try(valid_version(package.version));
  Try(valid_authors(package.authors));
  Try(valid_edition(package.edition));
  Try(valid_license(package.license));
  Try(valid_repository(package.repository));
  Try(valid_description(package.description));
  return Ok(package);
}

[[nodiscard]] auto
valid_profile(const Option<String>& profile, Option<bool> release)
    -> Result<Option<String>, String> {
  if (release.has_value() && release.value()) {
    if (profile.has_value() && profile.value() != "release") {
      return Err(format(
          "Specified profiles are conflicted: you specify --release and --profile={}.",
          profile.value()
      ));
    } else {
      return Ok("release");
    }
  } else {
    if (profile.has_value()) {
      return Ok(profile.value());
    } else {
      return Ok(None);
    }
  }
}

} // namespace poac::util::validator
