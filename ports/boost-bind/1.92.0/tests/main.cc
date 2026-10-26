// Exercises Boost.Bind: binding free functions, member functions, and data
// members with placeholders, plus boost::mem_fn.
#include <boost/bind/bind.hpp>
#include <boost/mem_fn.hpp>

#include <algorithm>
#include <vector>

static int sub(int a, int b) { return a - b; }

struct Counter {
  int value;
  int add(int n) { value += n; return value; }
  int get() const { return value; }
};

int main() {
  using namespace boost::placeholders;

  if (boost::bind(sub, _1, 3)(10) != 7) return 1;
  if (boost::bind(sub, _2, _1)(1, 5) != 4) return 1;
  if (boost::bind(sub, 8, 2)() != 6) return 1;

  Counter c = {1};
  boost::bind(&Counter::add, &c, _1)(4);
  if (c.value != 5) return 1;
  if (boost::bind(&Counter::value, _1)(c) != 5) return 1;

  std::vector<Counter> cs(3);
  for (int i = 0; i < 3; ++i) cs[i].value = i;
  if (std::count_if(cs.begin(), cs.end(), boost::bind(&Counter::get, _1) > 0) != 2) return 1;
  if (boost::mem_fn(&Counter::get)(cs[2]) != 2) return 1;

  std::vector<int> v(3, 10);
  std::transform(v.begin(), v.end(), v.begin(), boost::bind(sub, _1, 4));
  if (v[0] != 6 || v[2] != 6) return 1;
  return 0;
}
