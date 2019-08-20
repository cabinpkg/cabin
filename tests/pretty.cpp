#define BOOST_TEST_MAIN
#include <boost/test/included/unit_test.hpp>
#include <exception>
#include <string>
#include <utility>
#include <poac/util/pretty.hpp>

// std::string to_time(const std::string& s)
BOOST_AUTO_TEST_CASE( poac_util_pretty_to_time_test1 )
{
    using poac::util::pretty::to_time;

    BOOST_CHECK( to_time("0.5") == "0.5s" );
    BOOST_CHECK( to_time("1.1") == "1s" );
    BOOST_CHECK( to_time("60") == "1m 0s" );
    BOOST_CHECK( to_time("60.1") == "1m 0s" );
    BOOST_CHECK( to_time("60.5") == "1m 0s" );
    BOOST_CHECK( to_time("70") == "1m 10s" );
    BOOST_CHECK( to_time("3600") == "1h 0s" );
    BOOST_CHECK( to_time("3670") == "1h 1m 10s" );
    BOOST_CHECK( to_time("86400") == "1d 0s" );
    BOOST_CHECK( to_time("86470") == "1d 1m 10s" );
    BOOST_CHECK( to_time("90070") == "1d 1h 1m 10s" );
    BOOST_CHECK( to_time("900q70") == "15m 0s" ); // 900
    BOOST_CHECK_THROW( to_time("r90"), std::invalid_argument );
}

// std::pair<float, std::string> to_byte(const float b)
BOOST_AUTO_TEST_CASE( poac_util_pretty_to_byte_test1 )
{
    using poac::util::pretty::to_byte;

    // Implicit conversion, float to int
    std::pair<int, std::string> temp = to_byte(12);
    BOOST_CHECK( std::to_string(temp.first) + temp.second == "12B" );
    temp = to_byte(1'000);
    BOOST_CHECK( std::to_string(temp.first) + temp.second == "1KB" );
    temp = to_byte(12'000);
    BOOST_CHECK( std::to_string(temp.first) + temp.second == "12KB" );
    temp = to_byte(1'000'000);
    BOOST_CHECK( std::to_string(temp.first) + temp.second == "1MB" );
    temp = to_byte(12'000'000);
    BOOST_CHECK( std::to_string(temp.first) + temp.second == "12MB" );
    temp = to_byte(1'000'000'000);
    BOOST_CHECK( std::to_string(temp.first) + temp.second == "1GB" );
    temp = to_byte(12'000'000'000);
    BOOST_CHECK( std::to_string(temp.first) + temp.second == "12GB" );
    temp = to_byte(1'000'000'000'000);
    BOOST_CHECK( std::to_string(temp.first) + temp.second == "1TB" );
    temp = to_byte(12'000'000'000'000);
    BOOST_CHECK( std::to_string(temp.first) + temp.second == "12TB" );
    temp = to_byte(1'000'000'000'000'000);
    BOOST_CHECK( std::to_string(temp.first) + temp.second == "1000TB" );
    temp = to_byte(1'000'000'000'000'000'000);
    BOOST_CHECK( std::to_string(temp.first) + temp.second == "1000000TB" );
}

// std::string clip_string(const std::string& s, const unsigned long& n)
BOOST_AUTO_TEST_CASE( poac_util_pretty_clip_string_test1 )
{
    using poac::util::pretty::clip_string;

    BOOST_CHECK( clip_string("Long sentence", 15) == "Long sentence" );
    BOOST_CHECK( clip_string("Long sentence", 10) == "Long sente..." );
}
