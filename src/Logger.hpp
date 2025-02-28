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

enum class LogLevel : uint8_t {
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

class Logger {
  LogLevel level = LogLevel::Info;

  constexpr Logger() noexcept = default;

public:
  // Logger is a singleton
  constexpr Logger(const Logger&) = delete;
  constexpr Logger& operator=(const Logger&) = delete;
  constexpr Logger(Logger&&) noexcept = delete;
  constexpr Logger& operator=(Logger&&) noexcept = delete;
  constexpr ~Logger() noexcept = default;

  static Logger& instance() noexcept {
    static Logger instance;
    return instance;
  }
  static void setLevel(LogLevel level) noexcept {
    instance().level = level;
  }
  static LogLevel getLevel() noexcept {
    return instance().level;
  }

  template <typename... Args>
  static void error(fmt::format_string<Args...> fmt, Args&&... args) noexcept {
    logln(
        LogLevel::Error,
        [](const std::string_view head) noexcept {
          return Bold(Red(head)).toErrStr();
        },
        "Error: ", fmt, std::forward<Args>(args)...
    );
  }
  template <typename... Args>
  static void warn(fmt::format_string<Args...> fmt, Args&&... args) noexcept {
    logln(
        LogLevel::Warn,
        [](const std::string_view head) noexcept {
          return Bold(Yellow(head)).toErrStr();
        },
        "Warning: ", fmt, std::forward<Args>(args)...
    );
  }
  template <typename... Args>
  static void info(
      const std::string_view header, fmt::format_string<Args...> fmt,
      Args&&... args
  ) noexcept {
    constexpr int infoHeaderMaxLength = 12;
    constexpr int infoHeaderEscapeSequenceOffset = 11;
    logln(
        LogLevel::Info,
        [](const std::string_view head) noexcept {
          return fmt::format(
              "{:>{}} ", Bold(Green(head)).toErrStr(),
              shouldColorStderr()
                  ? infoHeaderMaxLength + infoHeaderEscapeSequenceOffset
                  : infoHeaderMaxLength
          );
        },
        header, fmt, std::forward<Args>(args)...
    );
  }
  template <typename Arg1, typename... Args>
  static void
  verbose(fmt::format_string<Args...> fmt, Arg1&& a1, Args&&... args) noexcept {
    logln(
        LogLevel::Verbose,
        [](const std::string_view head) noexcept { return head; },
        std::forward<Arg1>(a1), fmt, std::forward<Args>(args)...
    );
  }
  template <typename Arg1, typename... Args>
  static void veryVerbose(
      fmt::format_string<Args...> fmt, Arg1&& a1, Args&&... args
  ) noexcept {
    logln(
        LogLevel::Verbose,
        [](const std::string_view head) noexcept { return head; },
        std::forward<Arg1>(a1), fmt, std::forward<Args>(args)...
    );
  }

private:
  template <typename... Args>
  static void logln(
      LogLevel level, HeadProcessor auto&& processHead, auto&& head,
      fmt::format_string<Args...> fmt, Args&&... args
  ) noexcept {
    loglnImpl(
        level, std::forward<decltype(processHead)>(processHead),
        std::forward<decltype(head)>(head), fmt, std::forward<Args>(args)...
    );
  }

  template <typename... Args>
  static void loglnImpl(
      LogLevel level, HeadProcessor auto&& processHead, auto&& head,
      fmt::format_string<Args...> fmt, Args&&... args
  ) noexcept {
    instance().log(
        level, std::forward<decltype(processHead)>(processHead),
        std::forward<decltype(head)>(head), fmt, std::forward<Args>(args)...
    );
  }

  template <typename... Args>
  void
  log(LogLevel level, HeadProcessor auto&& processHead, auto&& head,
      fmt::format_string<Args...> fmt, Args&&... args) noexcept {
    if (level <= this->level) {
      fmt::print(
          stderr, "{}{}\n",
          std::invoke(
              std::forward<decltype(processHead)>(processHead),
              std::forward<decltype(head)>(head)
          ),
          fmt::format(fmt, std::forward<Args>(args)...)
      );
    }
  }
};

inline void
setLogLevel(LogLevel level) noexcept {
  Logger::setLevel(level);
}
inline LogLevel
getLogLevel() noexcept {
  return Logger::getLevel();
}

inline bool
isVerbose() noexcept {
  return getLogLevel() >= LogLevel::Verbose;
}
inline bool
isQuiet() noexcept {
  return getLogLevel() == LogLevel::Off;
}

}  // namespace cabin
