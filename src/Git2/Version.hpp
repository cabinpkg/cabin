#pragma once

#include <fmt/format.h>
#include <string>

namespace git2 {

struct Version {
  int major{};
  int minor{};
  int rev{};
  int features;

  Version();

  std::string toString() const;

  /// Returns true if libgit2 was built thread-aware and can be safely used
  /// from multiple threads.
  bool hasThread() const noexcept;

  /// Returns true if libgit2 was built with and linked against a TLS
  /// implementation.
  ///
  /// Custom TLS streams may still be added by the user to support HTTPS
  /// regardless of this.
  bool hasHttps() const noexcept;

  /// Returns true if libgit2 was built with and linked against libssh2.
  ///
  /// A custom transport may still be added by the user to support libssh2
  /// regardless of this.
  bool hasSsh() const noexcept;

  /// Returns true if libgit2 was built with support for sub-second
  /// resolution in file modification times.
  bool hasNsec() const noexcept;
};

} // namespace git2

template <>
struct fmt::formatter<git2::Version> : formatter<std::string> {
  auto format(const git2::Version& v, format_context& ctx) const
      -> format_context::iterator;
};
