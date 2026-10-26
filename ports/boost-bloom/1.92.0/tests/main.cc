// Exercises Boost.Bloom: boost::bloom::filter insertion, membership queries
// (no false negatives), clear, and capacity accessors.
#include <boost/bloom.hpp>

#include <cstddef>
#include <string>

int main() {
  typedef boost::bloom::filter<std::string, 5> filter;

  filter f(100000);
  if (f.capacity() < 100000) return 1;
  if (f.may_contain("hello")) return 1;  // an empty filter has no bits set

  f.insert("hello");
  f.insert("Boost");
  for (int i = 0; i < 100; ++i) f.insert(std::to_string(i));

  if (!f.may_contain("hello") || !f.may_contain("Boost")) return 1;
  for (int i = 0; i < 100; ++i)
    if (!f.may_contain(std::to_string(i))) return 1;

  typedef boost::bloom::filter<int, 1, boost::bloom::block<unsigned long long, 4> > block_filter;
  block_filter g(4096);
  g.insert(7);
  if (!g.may_contain(7)) return 1;

  std::size_t bits = f.capacity();
  f.clear();  // zeroes the bit array, keeping its capacity
  if (f.capacity() != bits || f.may_contain("hello")) return 1;
  return 0;
}
