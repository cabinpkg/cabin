// Exercises Boost.Algorithm: string algorithms (trim, case conversion, split,
// replace) and the C++11 sequence algorithms (all_of, any_of, hex).
#include <boost/algorithm/cxx11/all_of.hpp>
#include <boost/algorithm/cxx11/any_of.hpp>
#include <boost/algorithm/hex.hpp>
#include <boost/algorithm/string.hpp>

#include <string>
#include <vector>

static bool is_positive(int x) { return x > 0; }

int main() {
  std::string s = "  Hello, Boost World  ";
  boost::algorithm::trim(s);
  if (s != "Hello, Boost World") return 1;
  if (boost::algorithm::to_upper_copy(s) != "HELLO, BOOST WORLD") return 1;
  if (!boost::algorithm::starts_with(s, "Hello")) return 1;
  if (!boost::algorithm::iequals("BOOST", "boost")) return 1;

  std::vector<std::string> parts;
  boost::algorithm::split(parts, "a,b,,c", boost::algorithm::is_any_of(","));
  if (parts.size() != 4 || parts[0] != "a" || parts[2] != "" || parts[3] != "c") return 1;
  if (boost::algorithm::join(parts, "-") != "a-b--c") return 1;
  if (boost::algorithm::replace_all_copy(std::string("aXbXc"), "X", "--") != "a--b--c") return 1;

  std::vector<int> v;
  v.push_back(1);
  v.push_back(2);
  v.push_back(3);
  if (!boost::algorithm::all_of(v, is_positive)) return 1;
  if (!boost::algorithm::any_of_equal(v, 2)) return 1;
  if (boost::algorithm::any_of_equal(v, 7)) return 1;

  if (boost::algorithm::hex(std::string("AZ")) != "415A") return 1;
  if (boost::algorithm::unhex(std::string("415A")) != "AZ") return 1;
  return 0;
}
