#include "TermColor.hpp"

#include "Logger.hpp"
#include "Rustify.hpp"

#include <cstdlib>

static bool isTerm() noexcept {
  return std::getenv("TERM") != nullptr;
}

static ColorMode getColorMode(StringRef str) noexcept {
  if (str == "always") {
    return ColorMode::always;
  } else if (str == "auto") {
    return ColorMode::automatic;
  } else if (str == "never") {
    return ColorMode::never;
  } else {
    Logger::warn("unknown color mode `", str, "`; falling back to auto");
    return ColorMode::automatic;
  }
}

class ColorState {
public:
  void set(ColorMode mode) noexcept {
    switch (mode) {
      case ColorMode::always:
        should_color_ = true;
        return;
      case ColorMode::automatic:
        should_color_ = isTerm();
        return;
      case ColorMode::never:
        should_color_ = false;
        return;
    }
  }
  inline bool shouldColor() const noexcept {
    return should_color_;
  }

  static ColorState& instance() noexcept {
    static ColorState instance;
    return instance;
  }

private:
  // default: automatic
  bool should_color_;

  ColorState() noexcept {
    if (const char* color = std::getenv("POAC_TERM_COLOR")) {
      set(getColorMode(color));
    } else {
      should_color_ = isTerm();
    }
  }

  // Delete copy constructor and assignment operator to prevent copying
  ColorState(const ColorState&) = delete;
  ColorState& operator=(const ColorState&) = delete;
};

void setColorMode(ColorMode cm) noexcept {
  ColorState::instance().set(cm);
}

void setColorMode(StringRef str) noexcept {
  setColorMode(getColorMode(str));
}

bool shouldColor() noexcept {
  return ColorState::instance().shouldColor();
}

static String colorize(StringRef str, StringRef color) noexcept {
  if (!shouldColor()) {
    return String(str);
  }
  return String(color) + String(str) + "\033[0m";
}

String gray(StringRef str) noexcept {
  return colorize(str, "\033[30m");
}
String red(StringRef str) noexcept {
  return colorize(str, "\033[31m");
}
String green(StringRef str) noexcept {
  return colorize(str, "\033[32m");
}
String yellow(StringRef str) noexcept {
  return colorize(str, "\033[33m");
}
String blue(StringRef str) noexcept {
  return colorize(str, "\033[34m");
}
String magenta(StringRef str) noexcept {
  return colorize(str, "\033[35m");
}
String cyan(StringRef str) noexcept {
  return colorize(str, "\033[36m");
}

String bold(StringRef str) noexcept {
  return colorize(str, "\033[1m");
}
