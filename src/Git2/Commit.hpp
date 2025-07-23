#pragma once

#include "Global.hpp"
#include "Oid.hpp"
#include "Repository.hpp"
#include "Time.hpp"

#include <git2/commit.h>

namespace git2 {

struct Commit : public GlobalState {
  git_commit* raw = nullptr;

  Commit() = default;
  ~Commit();

  Commit(const Commit&) = delete;
  Commit(Commit&&) noexcept = default;
  Commit& operator=(const Commit&) = delete;
  Commit& operator=(Commit&&) noexcept = default;

  /// Lookup a commit object from a repository.
  Commit& lookup(const Repository& repo, const Oid& oid);

  /// Get the commit time (i.e. committer time) of a commit.
  Time time() const;
};

} // namespace git2
