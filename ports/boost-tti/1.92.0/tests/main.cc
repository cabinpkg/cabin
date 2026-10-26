// Boost.TTI: generated metafunctions detect nested types, member data, and member functions,
// and BOOST_TTI_MEMBER_TYPE extracts a nested type.
#include <boost/tti/tti.hpp>
#include <boost/static_assert.hpp>

BOOST_TTI_HAS_TYPE(value_type)
BOOST_TTI_HAS_MEMBER_DATA(count)
BOOST_TTI_HAS_MEMBER_FUNCTION(size)
BOOST_TTI_HAS_STATIC_MEMBER_FUNCTION(make)
BOOST_TTI_MEMBER_TYPE(value_type)

struct Box {
    typedef long value_type;
    int count;
    int size() const { return count; }
    static Box make() { Box b; b.count = 3; return b; }
};
struct Empty {};

BOOST_STATIC_ASSERT((has_type_value_type<Box>::value));
BOOST_STATIC_ASSERT((!has_type_value_type<Empty>::value));
BOOST_STATIC_ASSERT((has_member_data_count<int Box::*>::value));
BOOST_STATIC_ASSERT((!has_member_data_count<long Box::*>::value));
BOOST_STATIC_ASSERT((has_member_function_size<int (Box::*)() const>::value));
BOOST_STATIC_ASSERT((!has_member_function_size<Empty, int>::value));
BOOST_STATIC_ASSERT((has_static_member_function_make<Box, Box ()>::value));

template <class T>
int describe() {
    return (has_type_value_type<T>::value ? 1 : 0) + (has_member_function_size<int (T::*)() const>::value ? 2 : 0);
}

int main() {
    member_type_value_type<Box>::type v = 42L;
    if (sizeof(v) != sizeof(long) || v != 42L) return 1;
    if (!boost::tti::valid_member_metafunction<member_type_value_type<Box> >::value) return 1;
    if (boost::tti::valid_member_metafunction<member_type_value_type<Empty> >::value) return 1;
    if (describe<Box>() != 3 || describe<Empty>() != 0) return 1;
    return Box::make().size() == 3 ? 0 : 1;
}
