#include "TermColor.hpp"

#include "Logger.hpp"

#include <cstdlib>
#include <string>
#include <string_view>

namespace cabin {

static bool
isTerm() noexcept {
  return std::getenv("TERM") != nullptr;
}

static ColorMode
getColorMode(const std::string_view str) noexcept {
  if (str == "always") {
    return ColorMode::Always;
  } else if (str == "auto") {
    return ColorMode::Auto;
  } else if (str == "never") {
    return ColorMode::Never;
  } else {
    logger::warn("unknown color mode `{}`; falling back to auto", str);
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
  bool shouldColor() const noexcept {
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
    if (const char* color = std::getenv("CABIN_TERM_COLOR")) {
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
setColorMode(const std::string_view str) noexcept {
  setColorMode(getColorMode(str));
}

bool
shouldColor() noexcept {
  return ColorState::instance().shouldColor();
}

static std::string
colorize(const std::string_view str, const std::string_view code) noexcept {
  if (!shouldColor()) {
    return std::string(str);
  }

  std::string res;
  if (str.starts_with("\033[")) {
    const std::size_t end = str.find('m');
    if (end == std::string_view::npos) {
      // Invalid color escape sequence
      return std::string(str);
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

std::string
gray(const std::string_view str) noexcept {
  return colorize(str, "30");
}
std::string
red(const std::string_view str) noexcept {
  return colorize(str, "31");
}
std::string
green(const std::string_view str) noexcept {
  return colorize(str, "32");
}
std::string
yellow(const std::string_view str) noexcept {
  return colorize(str, "33");
}
std::string
blue(const std::string_view str) noexcept {
  return colorize(str, "34");
}
std::string
magenta(const std::string_view str) noexcept {
  return colorize(str, "35");
}
std::string
cyan(const std::string_view str) noexcept {
  return colorize(str, "36");
}

std::string
bold(const std::string_view str) noexcept {
  return colorize(str, "1");
}

}  // namespace cabin
