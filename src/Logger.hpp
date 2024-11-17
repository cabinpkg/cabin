#pragma once

#include "Rustify/Traits.hpp"
#include "TermColor.hpp"

#include <cstdint>
#include <fmt/core.h>
#include <functional>
#include <iomanip>
#include <iostream>
#include <ostream>
#include <source_location>
#include <string_view>
#include <type_traits>
#include <utility>

namespace logger {

enum class Level : uint8_t {
  Off = 0,  // --quiet, -q
  Error = 1,
  Warn = 2,
  Info = 3,   // default
  Debug = 4,  // --verbose, -v
  Trace = 5,  // -vv
};

// FIXME: duplicate code in Rustify/Tests.hpp but don't want to include it here.
// Maybe wait for modules to be stable?
constexpr std::string_view
prettifyFuncName(std::string_view func) noexcept {
  if (func.empty()) {
    return func;
  }

  const size_t end = func.find_last_of('(');
  if (end == std::string_view::npos) {
    return func;
  }
  func = func.substr(0, end);

  const size_t start = func.find_last_of(' ');
  if (start == std::string_view::npos) {
    return func;
  }
  return func.substr(start + 1);
}

template <typename Fn>
concept HeadProcessor = std::is_nothrow_invocable_v<Fn, std::string_view>
                        && Display<std::invoke_result_t<Fn, std::string_view>>;

class Logger {
  Level level = Level::Info;

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
  static void setLevel(Level level) noexcept {
    instance().level = level;
  }
  static Level getLevel() noexcept {
    return instance().level;
  }

  template <typename... Args>
  static void error(fmt::format_string<Args...> fmt, Args&&... args) noexcept {
    logln(
        Level::Error,
        [](const std::string_view head) noexcept { return bold(red(head)); },
        "Error: ", fmt, std::forward<Args>(args)...
    );
  }
  template <typename... Args>
  static void warn(fmt::format_string<Args...> fmt, Args&&... args) noexcept {
    logln(
        Level::Warn,
        [](const std::string_view head) noexcept { return bold(yellow(head)); },
        "Warning: ", fmt, std::forward<Args>(args)...
    );
  }
  template <typename... Args>
  static void info(
      const std::string_view header, fmt::format_string<Args...> fmt,
      Args&&... args
  ) noexcept {
    const int infoHeaderMaxLength = 12;
    const int infoHeaderEscapeSequenceOffset = 9;
    logln(
        Level::Info,
        [](const std::string_view head) noexcept {
          return fmt::format(
              "{:>{}} ", bold(green(head)),
              shouldColor()
                  ? infoHeaderMaxLength + infoHeaderEscapeSequenceOffset
                  : infoHeaderMaxLength
          );
        },
        header, fmt, std::forward<Args>(args)...
    );
  }
  template <typename... Args>
  static void debug(
      const std::string_view func, fmt::format_string<Args...> fmt,
      Args&&... args
  ) noexcept {
    debuglike(
        Level::Debug, blue("DEBUG"), func, fmt, std::forward<Args>(args)...
    );
  }
  template <typename... Args>
  static void trace(
      const std::string_view func, fmt::format_string<Args...> fmt,
      Args&&... args
  ) noexcept {
    debuglike(
        Level::Trace, cyan("TRACE"), func, fmt, std::forward<Args>(args)...
    );
  }

private:
  template <typename... Args>
  static void debuglike(
      Level level, const std::string_view lvlStr, const std::string_view func,
      fmt::format_string<Args...> fmt, Args&&... args
  ) noexcept {
    logln(
        level,
        [lvlStr](const std::string_view func) noexcept {
          return fmt::format(
              "{}Poac {} {}{} ", gray("["), lvlStr, prettifyFuncName(func),
              gray("]")
          );
        },
        func, fmt, std::forward<Args>(args)...
    );
  }

  template <typename... Args>
  static void logln(
      Level level, HeadProcessor auto&& processHead, auto&& head,
      fmt::format_string<Args...> fmt, Args&&... args
  ) noexcept {
    loglnImpl(
        std::cerr, level, std::forward<decltype(processHead)>(processHead),
        std::forward<decltype(head)>(head), fmt, std::forward<Args>(args)...
    );
  }

  template <typename... Args>
  static void loglnImpl(
      std::ostream& os, Level level, HeadProcessor auto&& processHead,
      auto&& head, fmt::format_string<Args...> fmt, Args&&... args
  ) noexcept {
    instance().log(
        os, level, std::forward<decltype(processHead)>(processHead),
        std::forward<decltype(head)>(head), fmt, std::forward<Args>(args)...
    );
  }

  template <typename... Args>
  void
  log(std::ostream& os, Level level, HeadProcessor auto&& processHead,
      auto&& head, fmt::format_string<Args...> fmt, Args&&... args) noexcept {
    if (level <= this->level) {
      os << std::invoke(
          std::forward<decltype(processHead)>(processHead),
          std::forward<decltype(head)>(head)
      );
      os << fmt::format(fmt, std::forward<Args>(args)...) << std::endl;
    }
  }
};

template <typename... Args>
inline void
error(fmt::format_string<Args...> fmt, Args&&... args) noexcept {
  Logger::error(fmt, std::forward<Args>(args)...);
}
template <typename... Args>
inline void
warn(fmt::format_string<Args...> fmt, Args&&... args) noexcept {
  Logger::warn(fmt, std::forward<Args>(args)...);
}
template <typename... Args>
inline void
info(
    const std::string_view header, fmt::format_string<Args...> fmt,
    Args&&... args
) noexcept {
  Logger::info(header, fmt, std::forward<Args>(args)...);
}

template <typename... Args>
struct debug {  // NOLINT(readability-identifier-naming)
  explicit debug(
      fmt::format_string<Args...> fmt, Args&&... args,
      const std::source_location& loc = std::source_location::current()
  ) noexcept {
    Logger::debug(loc.function_name(), fmt, std::forward<Args>(args)...);
  }
};
template <typename... Args>
debug(fmt::format_string<Args...>, Args&&...) -> debug<Args...>;

template <typename... Args>
struct trace {  // NOLINT(readability-identifier-naming)
  explicit trace(
      fmt::format_string<Args...> fmt, Args&&... args,
      const std::source_location& loc = std::source_location::current()
  ) noexcept {
    Logger::trace(loc.function_name(), fmt, std::forward<Args>(args)...);
  }
};
template <typename... Args>
trace(fmt::format_string<Args...>, Args&&...) -> trace<Args...>;

inline void
setLevel(Level level) noexcept {
  Logger::setLevel(level);
}
inline Level
getLevel() noexcept {
  return Logger::getLevel();
}

}  // namespace logger

inline bool
isVerbose() noexcept {
  return logger::getLevel() >= logger::Level::Debug;
}
inline bool
isQuiet() noexcept {
  return logger::getLevel() == logger::Level::Off;
}
