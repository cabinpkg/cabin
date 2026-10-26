// Exercises Boost.Convert's converters behind one convert<>() interface:
// lexical_cast, std::stream (manipulators), strtol (bases, width, fill,
// rounding through Boost.Math) and Spirit, with value_or() fallbacks on
// failed conversions and the apply<>() adaptor over a sequence.
#include <boost/convert.hpp>
#include <boost/convert/lexical_cast.hpp>
#include <boost/convert/spirit.hpp>
#include <boost/convert/stream.hpp>
#include <boost/convert/strtol.hpp>
#include <algorithm>
#include <functional>
#include <ios>
#include <iterator>
#include <string>
#include <vector>

namespace cnv = boost::cnv;
namespace arg = boost::cnv::parameter;
using boost::convert;
using std::string;

int main() {
    const cnv::lexical_cast lc;
    if (convert<int>("123", lc).value() != 123 || convert<int>("uhm", lc).value_or(-1) != -1) return 1;
    if (convert<string>(-45, lc).value() != "-45") return 2;

    cnv::cstream stream;
    if (convert<int>("   123", stream(std::skipws)).value() != 123) return 3;
    if (convert<int>("ff", stream(std::hex)).value_or(-1) != 255) return 4;

    // Converter settings persist, so each strtol starts fresh.
    typedef cnv::strtol strtol;
    if (convert<string>(254, strtol()(arg::base = cnv::base::hex)).value() != "FE") return 5;
    if (convert<int>("11111110", strtol()(arg::base = cnv::base::bin)).value() != 254) return 6;
    if (convert<string>(12, strtol()(arg::width = 5)(arg::fill = '*')).value() != "***12") return 7;
    if (convert<string>(-12.3451, strtol()(arg::precision = 2)).value() != "-12.35") return 8;
    if (convert<int>("12x", strtol()).value_or(-1) != -1) return 9;

    const cnv::spirit spirit;
    if (convert<double>("2.5", spirit).value() != 2.5 || convert<int>("x1", spirit)) return 10;

    const std::vector<string> in = {"1", "two", "3"};
    std::vector<int> out;
    std::transform(in.begin(), in.end(), std::back_inserter(out), cnv::apply<int>(std::cref(lc)).value_or(0));
    return out == std::vector<int>{1, 0, 3} ? 0 : 11;
}
