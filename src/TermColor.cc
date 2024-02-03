#include "TermColor.hpp"

#include "Logger.hpp"
#include "Rustify.hpp"

#include <cstdlib>

static bool
isTerm() noexcept {
  return std::getenv("TERM") != nullptr;
}

static ColorMode
getColorMode(const StringRef str) noexcept {
  if (str == "always") {
    return ColorMode::Always;
  } else if (str == "auto") {
    return ColorMode::Auto;
  } else if (str == "never") {
    return ColorMode::Never;
  } else {
    logger::warn("unknown color mode `", str, "`; falling back to auto");
    return ColorMode::Auto;
  }
}

struct ColorState {
  // ColorState is a singleton
  ColorState(const ColorState&) = delete;
  ColorState& operator=(const ColorState&) = delete;
  ColorState(ColorState&&) noexcept = delete;
  ColorState& operator=(ColorState&&) noexcept = delete;
  ~ColorState() noexcept = default;

  void set(const ColorMode mode) noexcept {
    switch (mode) {
      case ColorMode::Always:
        state = true;
        return;
      case ColorMode::Auto:
        state = isTerm();
        return;
      case ColorMode::Never:
        state = false;
        return;
    }
  }
  inline bool shouldColor() const noexcept {
    return state;
  }

  static ColorState& instance() noexcept {
    static ColorState instance;
    return instance;
  }

private:
  // default: automatic
  bool state;

  ColorState() noexcept {
    if (const char* color = std::getenv("POAC_TERM_COLOR")) {
      set(getColorMode(color));
    } else {
      state = isTerm();
    }
  }
};

void
setColorMode(const ColorMode mode) noexcept {
  ColorState::instance().set(mode);
}

void
setColorMode(const StringRef str) noexcept {
  setColorMode(getColorMode(str));
}

bool
shouldColor() noexcept {
  return ColorState::instance().shouldColor();
}

static String
colorize(const StringRef str, const StringRef code) noexcept {
  if (!shouldColor()) {
    return String(str);
  }

  String res;
  if (str.starts_with("\033[")) {
    const usize end = str.find('m');
    if (end == String::npos) {
      // Invalid color escape sequence
      return String(str);
    }

    res = str.substr(0, end);
    res += ";";
    res += code;
    res += str.substr(end);
  } else {
    res = "\033[";
    res += code;
    res += 'm';
    res += str;
  }

  if (!res.ends_with("\033[0m")) {
    res += "\033[0m";
  }
  return res;
}

String
gray(const StringRef str) noexcept {
  return colorize(str, "30");
}
String
red(const StringRef str) noexcept {
  return colorize(str, "31");
}
String
green(const StringRef str) noexcept {
  return colorize(str, "32");
}
String
yellow(const StringRef str) noexcept {
  return colorize(str, "33");
}
String
blue(const StringRef str) noexcept {
  return colorize(str, "34");
}
String
magenta(const StringRef str) noexcept {
  return colorize(str, "35");
}
String
cyan(const StringRef str) noexcept {
  return colorize(str, "36");
}

String
bold(const StringRef str) noexcept {
  return colorize(str, "1");
}
