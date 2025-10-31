#pragma once

#include "Global.hpp"
#include "Repository.hpp"

#include <git2/describe.h>
#include <string>
#include <string_view>

namespace git2 {

struct DescribeOptions : public GlobalState {
  git_describe_options raw{};

  DescribeOptions();
  ~DescribeOptions() noexcept = default;

  DescribeOptions(const DescribeOptions&) = default;
  DescribeOptions& operator=(const DescribeOptions&) = default;
  DescribeOptions(DescribeOptions&&) = default;
  DescribeOptions& operator=(DescribeOptions&&) = default;

  DescribeOptions& maxCandidatesTags(unsigned int max);

  /// Sets the reference lookup strategy
  ///
  /// This behaves like the `--tags` option to git-describe.
  DescribeOptions& describeTags();

  /// Sets the reference lookup strategy
  ///
  /// This behaves like the `--all` option to git-describe.
  DescribeOptions& describeAll();

  /// Indicates when calculating the distance from the matching tag or
  /// reference whether to only walk down the first-parent ancestry.
  DescribeOptions& onlyFollowFirstParent(bool follow);

  /// If no matching tag or reference is found whether a describe option would
  /// normally fail. This option indicates, however, that it will instead fall
  /// back to showing the full id of the commit.
  DescribeOptions& showCommitOidAsFallback(bool show);

  DescribeOptions& pattern(std::string_view pattern);
};

struct DescribeFormatOptions : public GlobalState {
  git_describe_format_options raw{};

  DescribeFormatOptions();
  ~DescribeFormatOptions() = default;

  DescribeFormatOptions(const DescribeFormatOptions&) = default;
  DescribeFormatOptions& operator=(const DescribeFormatOptions&) = default;
  DescribeFormatOptions(DescribeFormatOptions&&) = default;
  DescribeFormatOptions& operator=(DescribeFormatOptions&&) = default;

  /// Sets the size of the abbreviated commit id to use.
  ///
  /// The value is the lower bound for the length of the abbreviated string,
  /// and the default is 7.
  DescribeFormatOptions& abbreviatedSize(unsigned int size);

  /// Sets whether or not the long format is used even when a shorter name
  /// could be used.
  DescribeFormatOptions& alwaysUseLongFormat(bool longF);

  /// If the workdir is dirty and this is set, this string will be appended to
  /// the description string.
  DescribeFormatOptions& dirtySuffix(std::string_view suffix);
};

struct Describe : public GlobalState {
  git_describe_result* raw = nullptr;

  Describe() = default;
  ~Describe();

  Describe(const Describe&) = delete;
  Describe(Describe&&) noexcept = default;
  Describe& operator=(const Describe&) = delete;
  Describe& operator=(Describe&&) noexcept = default;

  Describe& workdir(const Repository& repo, DescribeOptions& opts);

  std::string format(const DescribeFormatOptions& opts) const;
};

} // namespace git2
