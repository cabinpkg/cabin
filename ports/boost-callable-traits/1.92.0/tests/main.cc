// Exercises Boost.CallableTraits: return_type, args, function_type,
// class_of, and qualifier queries on free and member function types.
#include <boost/callable_traits.hpp>

#include <tuple>
#include <type_traits>

namespace ct = boost::callable_traits;

struct Widget {
  int scale(double) const { return 2; }
};

static long free_fn(int, char) { return 0; }

int main() {
  typedef decltype(&Widget::scale) pmf;
  typedef decltype(&free_fn) fp;

  static_assert(std::is_same<ct::return_type_t<fp>, long>::value, "return_type");
  static_assert(std::is_same<ct::args_t<fp>, std::tuple<int, char> >::value, "args");
  static_assert(std::is_same<ct::function_type_t<fp>, long(int, char)>::value, "function_type");
  static_assert(std::is_same<ct::class_of_t<pmf>, Widget>::value, "class_of");
  static_assert(std::is_same<ct::return_type_t<pmf>, int>::value, "pmf return_type");
  static_assert(ct::is_const_member<pmf>::value, "is_const_member");
  static_assert(!ct::is_volatile_member<pmf>::value, "is_volatile_member");
  static_assert(std::is_same<ct::remove_member_const_t<pmf>, int (Widget::*)(double)>::value,
                "remove_member_const");

  ct::return_type_t<pmf> r = (Widget().*(&Widget::scale))(1.0);
  if (r != 2) return 1;
  if (std::tuple_size<ct::args_t<pmf> >::value != 2) return 1;  // Widget const&, double
  if (free_fn(1, 'a') != 0) return 1;
  return 0;
}
