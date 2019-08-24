#define BOOST_TEST_MAIN
#include <boost/test/included/unit_test.hpp>
#include <boost/test/output_test_stream.hpp>
#include <poac/util/termcolor2/io.hpp>

BOOST_AUTO_TEST_CASE( termcolor2_io_test )
{
    boost::test_tools::output_test_stream output;
    output << termcolor2::make_string("foo");

    BOOST_CHECK( !output.is_empty(false) );
    BOOST_CHECK( output.is_equal("foo") );
}
