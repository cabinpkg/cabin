#include "Version.hpp"

#include "Cli.hpp"
#include "CurlVersion.hpp"
#include "Diag.hpp"
#include "Git2/Version.hpp"
#include "Rustify/Result.hpp"

#include <cstdlib>
#include <fmt/format.h>
#include <string_view>

#ifndef CABIN_CABIN_PKG_VERSION
#  error "CABIN_CABIN_PKG_VERSION is not defined"
#endif

#ifndef CABIN_CABIN_COMMIT_SHORT_HASH
#  error "CABIN_CABIN_COMMIT_SHORT_HASH is not defined"
#else
#  define COMMIT_SHORT_HASH CABIN_CABIN_COMMIT_SHORT_HASH
#endif

#ifndef CABIN_CABIN_COMMIT_HASH
#  error "CABIN_CABIN_COMMIT_HASH is not defined"
#else
#  define COMMIT_HASH CABIN_CABIN_COMMIT_HASH
#endif

#ifndef CABIN_CABIN_COMMIT_DATE
#  error "CABIN_CABIN_COMMIT_DATE is not defined"
#else
#  define COMMIT_DATE CABIN_CABIN_COMMIT_DATE
#endif

#if defined(__GNUC__) && !defined(__clang__)
#  define COMPILER_VERSION "GCC " __VERSION__
#else
#  define COMPILER_VERSION __VERSION__
#endif

namespace cabin {

Result<void> versionMain(CliArgsView args) noexcept;

const Subcmd VERSION_CMD =  //
    Subcmd{ "version" }
        .setDesc("Show version information")
        .setMainFn(versionMain);

static consteval std::string_view commitInfo() noexcept {
  if (sizeof(COMMIT_SHORT_HASH) <= 1 && sizeof(COMMIT_DATE) <= 1) {
    return "";
  } else if (sizeof(COMMIT_SHORT_HASH) <= 1) {
    return " (" COMMIT_DATE ")";
  } else if (sizeof(COMMIT_DATE) <= 1) {
    return " (" COMMIT_SHORT_HASH ")";
  } else {
    return " (" COMMIT_SHORT_HASH " " COMMIT_DATE ")";
  }
}

static consteval char firstMonthChar(const std::string_view month) noexcept {
  return (month[0] == 'O' || month[0] == 'N' || month[0] == 'D') ? '1' : '0';
}
static consteval char secondMonthChar(const std::string_view month) noexcept {
  if (month[0] == 'J') {
    if (month[1] == 'a') {
      // Jan
      return '1';
    } else if (month[2] == 'n') {
      // Jun
      return '6';
    } else {
      // Jul
      return '7';
    }
  } else if (month[0] == 'F') {
    // Feb
    return '2';
  } else if (month[0] == 'M') {
    if (month[1] == 'a') {
      // Mar
      return '3';
    } else {
      // May
      return '5';
    }
  } else if (month[0] == 'A') {
    if (month[1] == 'p') {
      // Apr
      return '4';
    } else {
      // Aug
      return '8';
    }
  } else if (month[0] == 'S') {
    // Sep
    return '9';
  } else if (month[0] == 'O') {
    // Oct
    return '0';
  } else if (month[0] == 'N') {
    // Nov
    return '1';
  } else if (month[0] == 'D') {
    // Dec
    return '2';
  } else {
    // Error
    return '0';
  }
}

// NOLINTNEXTLINE(cppcoreguidelines-avoid-c-arrays,modernize-avoid-c-arrays)
static constinit const char COMPILE_DATE[] = {
  // Year
  __DATE__[7], __DATE__[8], __DATE__[9], __DATE__[10],

  '-',

  // Month
  firstMonthChar(__DATE__), secondMonthChar(__DATE__),

  '-',

  // Day
  __DATE__[4] == ' ' ? '0' : __DATE__[4], __DATE__[5],

  '\0'
};

Result<void> versionMain(const CliArgsView args) noexcept {
  // Parse args
  for (auto itr = args.begin(); itr != args.end(); ++itr) {
    const std::string_view arg = *itr;

    const auto control = Try(Cli::handleGlobalOpts(itr, args.end(), "version"));
    if (control == Cli::Return) {
      return Ok();
    } else if (control == Cli::Continue) {
      continue;
    }
    return VERSION_CMD.noSuchArg(arg);
  }

  fmt::print("cabin {}{}\n", CABIN_CABIN_PKG_VERSION, commitInfo());
  if (isVerbose()) {
    fmt::print(
        "release: {}\n"
        "commit-hash: {}\n"
        "commit-date: {}\n"
        "compiler: {}\n"
        "compile-date: {}\n"
        "libgit2: {}\n"
        "libcurl: {}\n",
        CABIN_CABIN_PKG_VERSION, COMMIT_HASH, COMMIT_DATE, COMPILER_VERSION,
        COMPILE_DATE, git2::Version(), curl::Version());
  }

  return Ok();
}

}  // namespace cabin
