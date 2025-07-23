#pragma once

#include <exception>
#include <git2/errors.h>
#include <git2/version.h>
#include <string>

namespace git2 {

// NOLINTBEGIN
#if (LIBGIT2_VER_MAJOR < 1) && (LIBGIT2_VER_MINOR < 28)
const git_error* git_error_last();
void git_error_clear();

enum git_error_t {
  GIT_ERROR_NONE = 0,
  GIT_ERROR_NOMEMORY,
  GIT_ERROR_OS,
  GIT_ERROR_INVALID,
  GIT_ERROR_REFERENCE,
  GIT_ERROR_ZLIB,
  GIT_ERROR_REPOSITORY,
  GIT_ERROR_CONFIG,
  GIT_ERROR_REGEX,
  GIT_ERROR_ODB,
  GIT_ERROR_INDEX,
  GIT_ERROR_OBJECT,
  GIT_ERROR_NET,
  GIT_ERROR_TAG,
  GIT_ERROR_TREE,
  GIT_ERROR_INDEXER,
  GIT_ERROR_SSL,
  GIT_ERROR_SUBMODULE,
  GIT_ERROR_THREAD,
  GIT_ERROR_STASH,
  GIT_ERROR_CHECKOUT,
  GIT_ERROR_FETCHHEAD,
  GIT_ERROR_MERGE,
  GIT_ERROR_SSH,
  GIT_ERROR_FILTER,
  GIT_ERROR_REVERT,
  GIT_ERROR_CALLBACK,
  GIT_ERROR_CHERRYPICK,
  GIT_ERROR_DESCRIBE,
  GIT_ERROR_REBASE,
  GIT_ERROR_FILESYSTEM,
  GIT_ERROR_PATCH,
  GIT_ERROR_WORKTREE,
  GIT_ERROR_SHA1
};
#endif
// NOLINTEND

struct Exception final : public std::exception {
  Exception();
  ~Exception() noexcept override = default;

  const char* what() const noexcept override;
  git_error_t category() const noexcept;

  Exception(const Exception&) = default;
  Exception(Exception&&) noexcept = default;
  Exception& operator=(const Exception&) = default;
  Exception& operator=(Exception&&) noexcept = default;

private:
  std::string msg = "git2-cpp: ";
  git_error_t cat{ GIT_ERROR_NONE };
};

int git2Throw(int ret);

} // namespace git2
