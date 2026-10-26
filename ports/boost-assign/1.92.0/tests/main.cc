// Exercises Boost.Assign: list_of / map_list_of container initialization and
// the += / insert() operators for filling standard containers.
#include <boost/assign.hpp>

#include <map>
#include <set>
#include <string>
#include <vector>

int main() {
  using namespace boost::assign;

  std::vector<int> v = list_of(1)(2)(3);
  if (v.size() != 3 || v[2] != 3) return 1;
  v += 4, 5, 6;
  if (v.size() != 6 || v.back() != 6) return 1;

  std::map<std::string, int> m = map_list_of("one", 1)("two", 2);
  if (m.size() != 2 || m["two"] != 2) return 1;
  insert(m)("three", 3);
  if (m["three"] != 3) return 1;

  std::set<int> s;
  s += 3, 1, 2, 1;
  if (s.size() != 3 || *s.begin() != 1) return 1;

  std::vector<int> r = list_of(10).repeat(2, 20)(30);
  if (r.size() != 4 || r[1] != 20 || r[2] != 20 || r[3] != 30) return 1;
  return 0;
}
