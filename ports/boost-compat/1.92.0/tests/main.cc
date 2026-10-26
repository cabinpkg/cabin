// Exercises Boost.Compat: invoke, bind_front, bind_back, function_ref,
// to_underlying, and integer_sequence backports.
#include <boost/compat/bind_back.hpp>
#include <boost/compat/bind_front.hpp>
#include <boost/compat/function_ref.hpp>
#include <boost/compat/integer_sequence.hpp>
#include <boost/compat/invoke.hpp>
#include <boost/compat/to_underlying.hpp>

#include <type_traits>

static int sub(int a, int b) { return a - b; }
static int twice(int a) { return 2 * a; }

struct Point {
  int x;
  int dot(int k) const { return x * k; }
};

enum class Color : unsigned char { red = 1, green = 2 };

static int apply(boost::compat::function_ref<int(int)> f, int v) { return f(v); }

int main() {
  Point p = {3};
  if (boost::compat::invoke(sub, 9, 4) != 5) return 1;
  if (boost::compat::invoke(&Point::dot, p, 2) != 6) return 1;
  if (boost::compat::invoke(&Point::x, p) != 3) return 1;

  if (boost::compat::bind_front(sub, 10)(3) != 7) return 1;
  if (boost::compat::bind_back(sub, 10)(3) != -7) return 1;

  if (apply(boost::compat::bind_front(sub, 1), 5) != -4) return 1;
  if (apply(twice, 21) != 42) return 1;

  static_assert(std::is_same<decltype(boost::compat::to_underlying(Color::red)), unsigned char>::value,
                "to_underlying type");
  if (boost::compat::to_underlying(Color::green) != 2) return 1;

  typedef boost::compat::make_index_sequence<4> seq;
  static_assert(std::is_same<seq, boost::compat::index_sequence<0, 1, 2, 3> >::value, "index_sequence");
  return 0;
}
