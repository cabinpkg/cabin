#pragma once

#include "Config.hpp"
#include "Global.hpp"
#include "Object.hpp"
#include "Oid.hpp"

#include <git2/clone.h>
#include <git2/repository.h>
#include <string>

namespace git2 {

struct Repository : public GlobalState {
  git_repository* raw = nullptr;

  Repository() = default;
  ~Repository();

  Repository(const Repository&) = delete;
  Repository(Repository&&) noexcept = default;
  Repository& operator=(const Repository&) = delete;
  Repository& operator=(Repository&&) noexcept = default;

  /// Attempt to open an already-existing repository at `path`.
  ///
  /// The path can point to either a normal or bare repository.
  Repository& open(const std::string& path);
  /// Attempt to open an already-existing bare repository at `path`.
  ///
  /// The path can point to only a bare repository.
  Repository& openBare(const std::string& path);

  /// Creates a new repository in the specified folder.
  ///
  /// This by default will create any necessary directories to create the
  /// repository, and it will read any user-specified templates when creating
  /// the repository. This behavior can be configured through `init_opts`.
  Repository& init(const std::string& path);
  /// Creates a new `--bare` repository in the specified folder.
  ///
  /// The folder must exist prior to invoking this function.
  Repository& initBare(const std::string& path);

  /// Check if path is ignored by the ignore rules.
  bool isIgnored(const std::string& path) const;

  /// Clone a remote repository.
  Repository& clone(const std::string& url, const std::string& path,
                    const git_clone_options* opts = nullptr);

  /// Find a single object, as specified by a revision string.
  Object revparseSingle(const std::string& spec) const;

  /// Make the repository HEAD directly point to the Commit.
  Repository& setHeadDetached(const Oid& oid);

  /// Checkout current HEAD
  Repository& checkoutHead(bool force = false);

  /// Lookup a reference by name and resolve immediately to OID.
  Oid refNameToId(const std::string& refname) const;

  /// Get the configuration file for this repository.
  ///
  /// If a configuration file has not been set, the default config set for
  /// the repository will be returned, including global and system
  /// configurations (if they are available).
  Config config() const;
};

}  // end namespace git2
