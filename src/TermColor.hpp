#pragma once

#include <cstdint>
#include <string>
#include <string_view>

namespace cabin {

enum class ColorMode : uint8_t {
  Always,
  Auto,
  Never,
};

void setColorMode(ColorMode mode) noexcept;
void setColorMode(std::string_view str) noexcept;
bool shouldColor() noexcept;

std::string gray(std::string_view str) noexcept;
std::string red(std::string_view str) noexcept;
std::string green(std::string_view str) noexcept;
std::string yellow(std::string_view str) noexcept;
std::string blue(std::string_view str) noexcept;
std::string magenta(std::string_view str) noexcept;
std::string cyan(std::string_view str) noexcept;

std::string bold(std::string_view str) noexcept;

}  // namespace cabin
