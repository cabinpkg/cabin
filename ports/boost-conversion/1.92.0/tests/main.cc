// Exercises Boost.Conversion: polymorphic_cast (throws on failure),
// polymorphic_downcast, polymorphic_pointer_downcast, and implicit_cast.
#include <boost/implicit_cast.hpp>
#include <boost/polymorphic_cast.hpp>
#include <boost/polymorphic_pointer_cast.hpp>

#include <memory>
#include <typeinfo>

struct Base {
  virtual ~Base() {}
};
struct Derived : Base {
  int value() const { return 7; }
};
struct Other : Base {};

int main() {
  Derived d;
  Base* b = &d;
  if (boost::polymorphic_cast<Derived*>(b)->value() != 7) return 1;
  if (boost::polymorphic_downcast<Derived*>(b) != &d) return 1;
  if (boost::polymorphic_downcast<Derived&>(*b).value() != 7) return 1;

  Other o;
  Base* ob = &o;
  bool threw = false;
  try {
    boost::polymorphic_cast<Derived*>(ob);
  } catch (const std::bad_cast&) {
    threw = true;
  }
  if (!threw) return 1;

  std::shared_ptr<Base> sp(new Derived);
  std::shared_ptr<Derived> sd = boost::polymorphic_pointer_downcast<Derived>(sp);
  if (!sd || sd->value() != 7) return 1;

  long l = boost::implicit_cast<long>(5);
  const Base* cb = boost::implicit_cast<const Base*>(&d);
  if (l != 5 || cb != &d) return 1;
  return 0;
}
