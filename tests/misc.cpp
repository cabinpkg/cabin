#define BOOST_TEST_MAIN
#include <boost/test/included/unit_test.hpp>

#include <string>
#include <vector>

#include <poac/util/misc.hpp>


// std::vector<std::string> split(const std::string& raw, const std::string& delim)
BOOST_AUTO_TEST_CASE( poac_util_misc_split_test )
{
    using poac::util::misc::split;

    const std::string test_case = "Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do eiusmod ...";

    std::vector<std::string> excepted{ "Lorem ipsum dolor sit amet", " consectetur adipiscing elit", " sed do eiusmod ..." };
    BOOST_CHECK( split(test_case, ",") == excepted );

    excepted = { "Lorem ", "psum dolor s", "t amet, consectetur ad", "p", "sc", "ng el", "t, sed do e", "usmod ..." };
    BOOST_CHECK( split(test_case, "i") == excepted );
}
