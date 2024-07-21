#include "Time.hpp"

#include <ctime>
#include <iomanip>
#include <string>

namespace git2 {

std::string
Time::toString() const {
  const auto time2 = static_cast<std::time_t>(time);
  std::tm* time3 = std::localtime(&time2);

  constexpr usize dateLen = 10; // YYYY-MM-DD
  std::string buffer(dateLen, '\0');
  if (std::strftime(
          buffer.data(), dateLen + 1, // null-terminator
          "%Y-%m-%d", time3
      )
      == 0) {
    return {};
  }
  return buffer;
}

} // namespace git2
