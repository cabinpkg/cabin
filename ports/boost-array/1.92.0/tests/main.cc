// Exercises Boost.Array: aggregate initialization, element access, iteration,
// bounds-checked at(), fill, swap, and comparison.
#include <boost/array.hpp>

#include <numeric>
#include <stdexcept>

int main() {
  boost::array<int, 4> a = {{1, 2, 3, 4}};
  if (a.size() != 4 || a.empty()) return 1;
  if (a[0] != 1 || a.back() != 4) return 1;
  if (std::accumulate(a.begin(), a.end(), 0) != 10) return 1;

  bool threw = false;
  try {
    a.at(4);
  } catch (const std::out_of_range&) {
    threw = true;
  }
  if (!threw) return 1;

  boost::array<int, 4> b;
  b.fill(7);
  if (b[3] != 7) return 1;
  a.swap(b);
  if (a[0] != 7 || b[0] != 1) return 1;
  if (!(b < a) || a == b) return 1;

  boost::array<int, 0> empty;
  if (!empty.empty() || empty.begin() != empty.end()) return 1;
  return 0;
}
