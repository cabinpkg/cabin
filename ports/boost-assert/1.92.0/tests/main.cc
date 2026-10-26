// Exercises Boost.Assert: BOOST_ASSERT/BOOST_ASSERT_MSG routed to a user
// handler via BOOST_ENABLE_ASSERT_HANDLER, BOOST_VERIFY, and BOOST_CURRENT_FUNCTION.
#define BOOST_ENABLE_ASSERT_HANDLER
#include <boost/assert.hpp>
#include <boost/current_function.hpp>

#include <cstring>
#include <string>

static int failures = 0;
static std::string last_expr;

namespace boost {
void assertion_failed(char const* expr, char const*, char const*, long) {
  ++failures;
  last_expr = expr;
}
void assertion_failed_msg(char const* expr, char const*, char const*, char const*, long) {
  ++failures;
  last_expr = expr;
}
}  // namespace boost

int main() {
  int x = 1;
  BOOST_ASSERT(x == 1);
  if (failures != 0) return 1;

  BOOST_ASSERT(x == 2);
  if (failures != 1 || last_expr != "x == 2") return 1;

  BOOST_ASSERT_MSG(x > 5, "x is small");
  if (failures != 2 || last_expr != "x > 5") return 1;

  int calls = 0;
  BOOST_VERIFY(++calls == 1);
  if (calls != 1 || failures != 2) return 1;

  if (std::strstr(BOOST_CURRENT_FUNCTION, "main") == 0) return 1;
  return 0;
}
