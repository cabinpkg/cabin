#include "Describe.hpp"

#include "Exception.hpp"
#include "Repository.hpp"

#include <git2/buffer.h>
#include <git2/deprecated.h>
#include <git2/describe.h>
#include <git2/version.h>
#include <string>
#include <string_view>

namespace git2 {

DescribeOptions::DescribeOptions() {
  git2Throw(
      git_describe_options_init(&this->raw, GIT_DESCRIBE_OPTIONS_VERSION));
}

DescribeOptions&
DescribeOptions::maxCandidatesTags(const unsigned int max) {
  this->raw.max_candidates_tags = max;
  return *this;
}

DescribeOptions&
DescribeOptions::describeTags() {
  this->raw.describe_strategy = GIT_DESCRIBE_TAGS;
  return *this;
}

DescribeOptions&
DescribeOptions::describeAll() {
  this->raw.describe_strategy = GIT_DESCRIBE_ALL;
  return *this;
}

DescribeOptions&
DescribeOptions::onlyFollowFirstParent(const bool follow) {
  this->raw.only_follow_first_parent = follow;
  return *this;
}

DescribeOptions&
DescribeOptions::showCommitOidAsFallback(const bool show) {
  this->raw.show_commit_oid_as_fallback = show;
  return *this;
}

DescribeOptions&
DescribeOptions::pattern(const std::string_view pattern) {
  this->raw.pattern = pattern.data();
  return *this;
}

DescribeFormatOptions::DescribeFormatOptions() {
  git2Throw(
#if (LIBGIT2_VER_MAJOR < 1) && (LIBGIT2_VER_MINOR < 99)
      git_describe_init_format_options(
#else
      git_describe_format_options_init(
#endif
          &this->raw, GIT_DESCRIBE_FORMAT_OPTIONS_VERSION));
}

DescribeFormatOptions&
DescribeFormatOptions::abbreviatedSize(const unsigned int size) {
  this->raw.abbreviated_size = size;
  return *this;
}

DescribeFormatOptions&
DescribeFormatOptions::alwaysUseLongFormat(const bool longF) {
  this->raw.always_use_long_format = longF;
  return *this;
}

DescribeFormatOptions&
DescribeFormatOptions::dirtySuffix(const std::string_view suffix) {
  this->raw.dirty_suffix = suffix.data();
  return *this;
}

Describe::~Describe() { git_describe_result_free(this->raw); }

Describe&
Describe::workdir(const Repository& repo, DescribeOptions& opts) {
  git2Throw(git_describe_workdir(&this->raw, repo.raw, &opts.raw));
  return *this;
}

std::string
Describe::format(const DescribeFormatOptions& opts) const {
  git_buf ret = { nullptr, 0, 0 };
  git2Throw(git_describe_format(&ret, this->raw, &opts.raw));
  return { ret.ptr, ret.size };
}

}  // namespace git2
