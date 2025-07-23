#include "Commit.hpp"

#include "Exception.hpp"
#include "Oid.hpp"
#include "Repository.hpp"
#include "Time.hpp"

#include <git2/commit.h>

namespace git2 {

Commit::~Commit() { git_commit_free(this->raw); }

Commit& Commit::lookup(const Repository& repo, const Oid& oid) {
  git2Throw(git_commit_lookup(&this->raw, repo.raw, oid.raw));
  return *this;
}

Time Commit::time() const { return { git_commit_time(this->raw) }; }

}  // namespace git2
