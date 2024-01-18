#pragma once

#include "Rustify.hpp"
#include "TermColor.hpp"

#include <iomanip>
#include <iostream>
#include <utility>

enum class LogLevel : u8 {
  off = 0, // --quiet
  error = 1,
  warning = 2,
  info = 3, // default
  debug = 4 // --verbose
};

struct Logger {
  // Logger is a singleton
  Logger(const Logger&) = delete;
  Logger& operator=(const Logger&) = delete;
  Logger(Logger&&) noexcept = delete;
  Logger& operator=(Logger&&) noexcept = delete;
  ~Logger() noexcept = default;

  static Logger& instance() noexcept;
  static void setLevel(LogLevel) noexcept;
  static LogLevel getLevel() noexcept;

  template <typename... Args>
    requires(Display<Args> && ...)
  static void error(Args&&... message) noexcept {
    logln(std::cerr, LogLevel::error, std::forward<Args>(message)...);
  }
  template <typename... Args>
    requires(Display<Args> && ...)
  static void warn(Args&&... message) noexcept {
    logln(std::cerr, LogLevel::warning, std::forward<Args>(message)...);
  }
  template <typename T, typename... Args>
    requires(Display<T> && (Display<Args> && ...))
  static void info(T&& header, Args&&... message) noexcept {
    logln(
        std::cerr, LogLevel::info, std::forward<T>(header),
        std::forward<Args>(message)...
    );
  }
  template <typename... Args>
    requires(Display<Args> && ...)
  static void debug(Args&&... message) noexcept {
    logln(std::cerr, LogLevel::debug, std::forward<Args>(message)...);
  }

  template <typename T, typename... Args>
    requires(Display<T> && (Display<Args> && ...))
  static void logln(
      std::ostream& os, LogLevel messageLevel, T&& header, Args&&... message
  ) noexcept {
    log(os, messageLevel, std::forward<T>(header),
        std::forward<Args>(message)..., '\n');
  }
  template <typename T, typename... Args>
    requires(Display<T> && (Display<Args> && ...))
  static void
  log(std::ostream& os, LogLevel messageLevel, T&& header,
      Args&&... message) noexcept {
    instance().logImpl(
        os, messageLevel, std::forward<T>(header),
        std::forward<Args>(message)...
    );
  }

  template <typename T, typename... Args>
    requires(Display<T> && (Display<Args> && ...))
  void logImpl(
      std::ostream& os, LogLevel messageLevel, T&& header, Args&&... message
  ) noexcept {
    // For other than `info`, header means just the first argument.  For
    // `info`, header means its header.

    if (messageLevel <= level) {
      switch (messageLevel) {
        case LogLevel::off:
          return;
        case LogLevel::error:
          os << bold(red("Error: ")) << std::forward<T>(header);
          break;
        case LogLevel::warning:
          os << bold(yellow("Warning: ")) << std::forward<T>(header);
          break;
        case LogLevel::info:
          if (shouldColor()) {
            os << std::right << std::setw(27)
               << bold(green(std::forward<T>(header))) << ' ';
          } else {
            os << std::right << std::setw(12) << std::forward<T>(header) << ' ';
          }
          break;
        case LogLevel::debug:
          os << "[Poac] " << std::forward<T>(header);
          break;
      }
      (os << ... << std::forward<Args>(message));
      os << std::flush;
    }
  }

private:
  LogLevel level = LogLevel::info;

  Logger() noexcept = default;
};

bool isVerbose() noexcept;
bool isQuiet() noexcept;
