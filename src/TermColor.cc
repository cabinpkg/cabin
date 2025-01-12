#include "TermColor.hpp"

#include "Logger.hpp"

#include <cstdio>
#include <cstdlib>
#include <iostream>
#include <string_view>
#include <unistd.h>

namespace cabin {

enum class ColorMode : uint8_t {
  Always,
  Auto,
  Never,
};

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
    this->mode = mode;
  }
  ColorMode get() const noexcept {
    return mode;
  }

  static ColorState& instance() noexcept {
    static ColorState instance;
    return instance;
  }

private:
  ColorMode mode;

  ColorState() noexcept {
    if (const char* color = std::getenv("CABIN_TERM_COLOR")) {
      mode = getColorMode(color);
    } else {
      mode = ColorMode::Auto;
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

ColorMode
getColorMode() noexcept {
  return ColorState::instance().get();
}

static bool
isTerm(const std::ostream& os) noexcept {
  if (&os == &std::cout) {
    return isatty(fileno(stdout));
  } else if (&os == &std::cerr) {
    return isatty(fileno(stderr));
  }
  return false;
}

bool
shouldColor(const std::ostream& os) noexcept {
  switch (getColorMode()) {
    case ColorMode::Always:
      return true;
    case ColorMode::Auto:
      return isTerm(os);
    case ColorMode::Never:
      return false;
  }
}
bool
shouldColorStdout() noexcept {
  return shouldColor(std::cout);
}
bool
shouldColorStderr() noexcept {
  return shouldColor(std::cerr);
}

}  // namespace cabin
