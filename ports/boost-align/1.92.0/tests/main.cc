// Exercises Boost.Align: aligned_alloc/aligned_free, align_up/align_down,
// is_aligned, and the alignment_of trait.
#include <boost/align.hpp>

#include <cstddef>

struct Pair {
  char c;
  double d;
};

int main() {
  const std::size_t alignment = 64;
  void* p = boost::alignment::aligned_alloc(alignment, 100);
  if (p == 0) return 1;
  bool ok = boost::alignment::is_aligned(p, alignment);
  boost::alignment::aligned_free(p);
  if (!ok) return 1;

  if (boost::alignment::align_up(static_cast<std::size_t>(13), 8) != 16) return 1;
  if (boost::alignment::align_down(static_cast<std::size_t>(13), 8) != 8) return 1;
  if (!boost::alignment::is_aligned(static_cast<std::size_t>(32), 16)) return 1;
  if (boost::alignment::is_aligned(static_cast<std::size_t>(36), 16)) return 1;

  if (boost::alignment::alignment_of<char>::value != 1) return 1;
  if (boost::alignment::alignment_of<Pair>::value < boost::alignment::alignment_of<char>::value)
    return 1;

  char buffer[128];
  void* ptr = buffer;
  std::size_t space = sizeof buffer;
  void* r = boost::alignment::align(16, 32, ptr, space);
  if (r == 0 || !boost::alignment::is_aligned(r, 16)) return 1;
  return 0;
}
