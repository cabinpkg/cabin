// Exercises Boost.Spirit: Qi parses key=value pairs into a std::map and
// evaluates arithmetic through recursive rules with Phoenix semantic
// actions, and Karma generates a delimited list.
#include <boost/fusion/include/std_pair.hpp>
#include <boost/phoenix/operator.hpp>
#include <boost/spirit/include/karma.hpp>
#include <boost/spirit/include/qi.hpp>
#include <iterator>
#include <map>
#include <string>
#include <vector>

namespace qi = boost::spirit::qi;
namespace karma = boost::spirit::karma;
namespace ascii = boost::spirit::ascii;

typedef std::string::const_iterator iterator;

int main() {
    const std::string pairs = "alpha = 1, beta=22 ,gamma= 333";
    std::map<std::string, int> kv;
    iterator first = pairs.begin();
    if (!qi::phrase_parse(first, pairs.end(), (qi::lexeme[+ascii::alpha] >> '=' >> qi::int_) % ',',
                          ascii::space, kv) ||
        first != pairs.end() || kv.size() != 3 || kv["beta"] != 22)
        return 1;

    using qi::_1;
    using qi::_val;
    qi::rule<iterator, int(), ascii::space_type> expr, term, factor;
    expr = term[_val = _1] >> *('+' >> term[_val += _1] | '-' >> term[_val -= _1]);
    term = factor[_val = _1] >> *('*' >> factor[_val *= _1] | '/' >> factor[_val /= _1]);
    factor = qi::int_[_val = _1] | '(' >> expr[_val = _1] >> ')' | '-' >> factor[_val = -_1];
    const std::string calc = "2 * (3 + 4) - 10 / -5";
    int value = 0;
    first = calc.begin();
    if (!qi::phrase_parse(first, calc.end(), expr, ascii::space, value) || first != calc.end() || value != 16)
        return 2;

    std::string out;
    std::back_insert_iterator<std::string> sink(out);
    const std::vector<int> values = {1, 22, 333};
    if (!karma::generate(sink, karma::int_ % ", ", values)) return 3;
    return out == "1, 22, 333" ? 0 : 4;
}
