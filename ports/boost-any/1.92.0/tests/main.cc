// Exercises Boost.Any: storing values of different types in boost::any and
// retrieving them with any_cast, including failed casts and bad_any_cast.
#include <boost/any.hpp>

#include <string>
#include <typeinfo>
#include <vector>

int main() {
  boost::any a;
  if (!a.empty()) return 1;

  a = 42;
  if (a.empty() || a.type() != typeid(int)) return 1;
  if (boost::any_cast<int>(a) != 42) return 1;
  if (boost::any_cast<double>(&a) != nullptr) return 1;

  a = std::string("boost");
  if (boost::any_cast<std::string&>(a) != "boost") return 1;

  bool threw = false;
  try {
    boost::any_cast<int>(a);
  } catch (const boost::bad_any_cast&) {
    threw = true;
  }
  if (!threw) return 1;

  std::vector<boost::any> values;
  values.push_back(1);
  values.push_back(std::string("two"));
  values.push_back(3.0);
  if (boost::any_cast<std::string>(values[1]) != "two") return 1;

  boost::any copy = values[2];
  if (boost::any_cast<double>(copy) != 3.0) return 1;
  copy.clear();
  if (!copy.empty()) return 1;
  return 0;
}
