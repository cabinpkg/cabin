#pragma once

#include "TermColor.hpp"

#include <cstdint>
#include <cstdio>
#include <fmt/core.h>
#include <functional>
#include <string_view>
#include <type_traits>
#include <utility>

namespace cabin {

enum class DiagLevel : uint8_t {
  Off = 0,  // --quiet, -q
  Error = 1,
  Warn = 2,
  Info = 3,         // default
  Verbose = 4,      // --verbose, -v
  VeryVerbose = 5,  // -vv
};

template <typename Fn>
concept HeadProcessor =
    std::is_nothrow_invocable_v<Fn, std::string_view>
    && fmt::is_formattable<std::invoke_result_t<Fn, std::string_view>>::value;

class Diag {
  DiagLevel level = DiagLevel::Info;

  constexpr Diag() noexcept = default;

public:
  // Diag is a singleton
  constexpr Diag(const Diag&) = delete;
  constexpr Diag& operator=(const Diag&) = delete;
  constexpr Diag(Diag&&) noexcept = delete;
  constexpr Diag& operator=(Diag&&) noexcept = delete;
  constexpr ~Diag() noexcept = default;

  static Diag& instance() noexcept {
    static Diag instance;
    return instance;
  }
  static void setLevel(DiagLevel level) noexcept { instance().level = level; }
  static DiagLevel getLevel() noexcept { return instance().level; }

  template <typename... Args>
  static void error(fmt::format_string<Args...> fmt, Args&&... args) noexcept {
    logln(
        DiagLevel::Error,
        [](const std::string_view head) noexcept {
          return Bold(Red(head)).toErrStr();
        },
        "Error: ", fmt, std::forward<Args>(args)...);
  }
  template <typename... Args>
  static void warn(fmt::format_string<Args...> fmt, Args&&... args) noexcept {
    logln(
        DiagLevel::Warn,
        [](const std::string_view head) noexcept {
          return Bold(Yellow(head)).toErrStr();
        },
        "Warning: ", fmt, std::forward<Args>(args)...);
  }
  template <typename... Args>
  static void info(const std::string_view header,
                   fmt::format_string<Args...> fmt, Args&&... args) noexcept {
    constexpr int infoHeaderMaxLength = 12;
    constexpr int infoHeaderEscapeSequenceOffset = 11;
    logln(
        DiagLevel::Info,
        [](const std::string_view head) noexcept {
          return fmt::format("{:>{}} ", Bold(Green(head)).toErrStr(),
                             shouldColorStderr()
                                 ? infoHeaderMaxLength
                                       + infoHeaderEscapeSequenceOffset
                                 : infoHeaderMaxLength);
        },
        header, fmt, std::forward<Args>(args)...);
  }
  template <typename Arg1, typename... Args>
  static void verbose(fmt::format_string<Args...> fmt, Arg1&& a1,
                      Args&&... args) noexcept {
    logln(
        DiagLevel::Verbose,
        [](const std::string_view head) noexcept { return head; },
        std::forward<Arg1>(a1), fmt, std::forward<Args>(args)...);
  }
  template <typename Arg1, typename... Args>
  static void veryVerbose(fmt::format_string<Args...> fmt, Arg1&& a1,
                          Args&&... args) noexcept {
    logln(
        DiagLevel::Verbose,
        [](const std::string_view head) noexcept { return head; },
        std::forward<Arg1>(a1), fmt, std::forward<Args>(args)...);
  }

private:
  template <typename... Args>
  static void logln(DiagLevel level, HeadProcessor auto&& processHead,
                    auto&& head, fmt::format_string<Args...> fmt,
                    Args&&... args) noexcept {
    loglnImpl(level, std::forward<decltype(processHead)>(processHead),
              std::forward<decltype(head)>(head), fmt,
              std::forward<Args>(args)...);
  }

  template <typename... Args>
  static void loglnImpl(DiagLevel level, HeadProcessor auto&& processHead,
                        auto&& head, fmt::format_string<Args...> fmt,
                        Args&&... args) noexcept {
    instance().log(level, std::forward<decltype(processHead)>(processHead),
                   std::forward<decltype(head)>(head), fmt,
                   std::forward<Args>(args)...);
  }

  template <typename... Args>
  void log(DiagLevel level, HeadProcessor auto&& processHead, auto&& head,
           fmt::format_string<Args...> fmt, Args&&... args) noexcept {
    if (level <= this->level) {
      fmt::print(stderr, "{}{}\n",
                 std::invoke(std::forward<decltype(processHead)>(processHead),
                             std::forward<decltype(head)>(head)),
                 fmt::format(fmt, std::forward<Args>(args)...));
    }
  }
};

inline void
setDiagLevel(DiagLevel level) noexcept {
  Diag::setLevel(level);
}
inline DiagLevel
getDiagLevel() noexcept {
  return Diag::getLevel();
}

inline bool
isVerbose() noexcept {
  return getDiagLevel() >= DiagLevel::Verbose;
}
inline bool
isQuiet() noexcept {
  return getDiagLevel() == DiagLevel::Off;
}

}  // namespace cabin
