// Classifies and decomposes function, function-pointer, and member-function
// types with Boost.FunctionTypes at compile time.
#include <boost/function_types/function_arity.hpp>
#include <boost/function_types/is_function_pointer.hpp>
#include <boost/function_types/is_member_function_pointer.hpp>
#include <boost/function_types/result_type.hpp>
#include <boost/static_assert.hpp>

namespace ft = boost::function_types;

template <class A, class B> struct same { static const bool value = false; };
template <class A> struct same<A, A> { static const bool value = true; };

struct Widget {};

typedef long (*FnPtr)(char, short, int);
typedef double (Widget::*MemFn)(int, float) const;

BOOST_STATIC_ASSERT(ft::function_arity<FnPtr>::value == 3);
BOOST_STATIC_ASSERT(ft::function_arity<MemFn>::value == 3); // includes the object
BOOST_STATIC_ASSERT((same<ft::result_type<FnPtr>::type, long>::value));
BOOST_STATIC_ASSERT((same<ft::result_type<MemFn>::type, double>::value));
BOOST_STATIC_ASSERT(ft::is_function_pointer<FnPtr>::value);
BOOST_STATIC_ASSERT(!ft::is_function_pointer<MemFn>::value);
BOOST_STATIC_ASSERT(ft::is_member_function_pointer<MemFn>::value);

int main() {
    if (ft::function_arity<void(int)>::value != 1) return 1;
    if (ft::is_function_pointer<int>::value) return 1;
    return 0;
}
