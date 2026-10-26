// Exercises Boost.ContainerHash: boost::hash for strings, containers, and
// pairs, plus hash_combine/hash_range and a user hash_value overload.
#include <boost/container_hash/hash.hpp>

#include <cstddef>
#include <string>
#include <utility>
#include <vector>

namespace geo {
struct Point {
  int x, y;
};
inline std::size_t hash_value(const Point& p) {
  std::size_t seed = 0;
  boost::hash_combine(seed, p.x);
  boost::hash_combine(seed, p.y);
  return seed;
}
}  // namespace geo

int main() {
  boost::hash<std::string> hs;
  std::string s = "boost";
  if (hs(s) != boost::hash_range(s.begin(), s.end())) return 1;
  if (hs("boost") == hs("tsoob")) return 1;

  std::vector<int> v;
  v.push_back(1);
  v.push_back(2);
  v.push_back(3);
  if (boost::hash<std::vector<int> >()(v) != boost::hash_range(v.begin(), v.end())) return 1;

  std::size_t seed = 0;
  boost::hash_combine(seed, 1);
  boost::hash_combine(seed, 2);
  boost::hash_combine(seed, 3);
  if (seed != boost::hash_range(v.begin(), v.end())) return 1;

  geo::Point a = {1, 2}, b = {2, 1};
  boost::hash<geo::Point> hp;
  if (hp(a) != geo::hash_value(a) || hp(a) == hp(b)) return 1;

  std::pair<int, std::string> pr(1, "x");
  seed = 0;
  boost::hash_combine(seed, pr.first);
  boost::hash_combine(seed, pr.second);
  if (boost::hash<std::pair<int, std::string> >()(pr) != seed) return 1;
  return 0;
}
