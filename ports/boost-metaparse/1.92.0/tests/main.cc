// Exercises Boost.Metaparse building compile-time parsers over a compile-time
// string, checked with static assertions and the parsed value at runtime.
#include <boost/metaparse/build_parser.hpp>
#include <boost/metaparse/entire_input.hpp>
#include <boost/metaparse/int_.hpp>
#include <boost/metaparse/lit_c.hpp>
#include <boost/metaparse/middle_of.hpp>
#include <boost/metaparse/string.hpp>
#include <boost/static_assert.hpp>

using namespace boost::metaparse;

typedef build_parser<entire_input<int_> > int_parser;
typedef build_parser<entire_input<middle_of<lit_c<'('>, int_, lit_c<')'> > > > paren_parser;

typedef int_parser::apply<string<'1', '2', '3'> >::type parsed;
typedef paren_parser::apply<string<'(', '4', '2', ')'> >::type parsed_paren;

BOOST_STATIC_ASSERT(parsed::value == 123);
BOOST_STATIC_ASSERT(parsed_paren::value == 42);

int main() {
    if (parsed::value + parsed_paren::value != 165) return 1;
    return 0;
}
