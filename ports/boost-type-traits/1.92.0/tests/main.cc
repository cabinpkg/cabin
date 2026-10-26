// Boost.TypeTraits: classify and transform types at compile time, and dispatch on traits at runtime.
#include <boost/type_traits.hpp>
#include <boost/static_assert.hpp>

struct Base {};
struct Derived : Base {};

BOOST_STATIC_ASSERT((boost::is_same<boost::remove_const<const int>::type, int>::value));
BOOST_STATIC_ASSERT((boost::is_same<boost::remove_reference<int&>::type, int>::value));
BOOST_STATIC_ASSERT((boost::is_same<boost::add_pointer<int>::type, int*>::value));
BOOST_STATIC_ASSERT((boost::is_same<boost::conditional<true, char, long>::type, char>::value));
BOOST_STATIC_ASSERT((boost::is_base_of<Base, Derived>::value));
BOOST_STATIC_ASSERT((!boost::is_base_of<Derived, Base>::value));
BOOST_STATIC_ASSERT((boost::is_convertible<Derived*, Base*>::value));
BOOST_STATIC_ASSERT(boost::is_pointer<int*>::value && !boost::is_pointer<int>::value);
BOOST_STATIC_ASSERT(boost::is_integral<char>::value && boost::is_floating_point<double>::value);
BOOST_STATIC_ASSERT(boost::alignment_of<double>::value >= 1);
BOOST_STATIC_ASSERT(boost::extent<int[4][5], 1>::value == 5);

template <class T>
int category(const T&) {
    return boost::is_integral<T>::value ? 1 : boost::is_floating_point<T>::value ? 2 : boost::is_pointer<T>::value ? 3 : 0;
}

int main() {
    int i = 0;
    if (category(i) != 1 || category(1.5) != 2 || category(&i) != 3 || category(Base()) != 0) return 1;
    if (boost::is_unsigned<unsigned>::value != true || boost::is_signed<unsigned>::value) return 1;
    boost::integral_constant<int, 7> seven;
    return seven() == 7 ? 0 : 1;
}
