#pragma once

#include "Builder/Compiler.hpp"
#include "VersionReq.hpp"

#include <filesystem>
#include <optional>
#include <rs/result.hpp>
#include <string>
#include <utility>
#include <variant>

namespace cabin {

struct GitDependency {
  const std::string name;
  const std::string url;
  const std::optional<std::string> target;

  [[nodiscard]] std::filesystem::path installDir() const;
  rs::Result<CompilerOpts> install() const;

  GitDependency(std::string name, std::string url,
                std::optional<std::string> target)
      : name(std::move(name)), url(std::move(url)), target(std::move(target)) {}
};

struct PathDependency {
  const std::string name;
  const std::string path;

  rs::Result<CompilerOpts> install() const;

  PathDependency(std::string name, std::string path)
      : name(std::move(name)), path(std::move(path)) {}
};

struct SystemDependency {
  const std::string name;
  const VersionReq versionReq;

  rs::Result<CompilerOpts> install() const;

  SystemDependency(std::string name, VersionReq versionReq)
      : name(std::move(name)), versionReq(std::move(versionReq)) {}
};

using Dependency =
    std::variant<GitDependency, PathDependency, SystemDependency>;

} // namespace cabin
