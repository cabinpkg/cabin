#pragma once

#include "Rustify/Result.hpp"
#include "VersionReq.hpp"

#include <optional>
#include <string>
#include <utility>
#include <variant>

namespace cabin {

struct DepMetadata {
  const std::string includes;  // -Isomething
  const std::string libs;      // -Lsomething -lsomething
};

struct GitDependency {
  const std::string name;
  const std::string url;
  const std::optional<std::string> target;

  Result<DepMetadata> install() const;

  GitDependency(
      std::string name, std::string url, std::optional<std::string> target
  )
      : name(std::move(name)), url(std::move(url)), target(std::move(target)) {}
};

struct PathDependency {
  const std::string name;
  const std::string path;

  Result<DepMetadata> install() const;

  PathDependency(std::string name, std::string path)
      : name(std::move(name)), path(std::move(path)) {}
};

struct SystemDependency {
  const std::string name;
  const VersionReq versionReq;

  Result<DepMetadata> install() const;

  SystemDependency(std::string name, VersionReq versionReq)
      : name(std::move(name)), versionReq(std::move(versionReq)) {};
};

using Dependency =
    std::variant<GitDependency, PathDependency, SystemDependency>;

}  // namespace cabin
