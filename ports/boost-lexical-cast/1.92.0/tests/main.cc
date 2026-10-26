// Exercises Boost.LexicalCast conversions in both directions, the
// bad_lexical_cast failure path, and the non-throwing try_lexical_convert.
#include <boost/lexical_cast.hpp>

#include <string>
#include <typeinfo>

int main() {
    if (boost::lexical_cast<int>("-42") != -42) return 1;
    if (boost::lexical_cast<unsigned>(std::string("7")) != 7u) return 1;
    if (boost::lexical_cast<double>("2.5") != 2.5) return 1;
    if (boost::lexical_cast<std::string>(1234) != "1234") return 1;
    if (boost::lexical_cast<std::string>(0.5) != "0.5") return 1;
    if (boost::lexical_cast<int>("123456", 3) != 123) return 1;
    if (boost::lexical_cast<char>("x") != 'x' || !boost::lexical_cast<bool>("1")) return 1;

    try {
        boost::lexical_cast<int>("12x");
        return 1;
    } catch (const boost::bad_lexical_cast& e) {
        if (e.target_type() != typeid(int)) return 1;
    }

    int out = 0;
    if (!boost::conversion::try_lexical_convert("99", out) || out != 99) return 1;
    if (boost::conversion::try_lexical_convert("abc", out)) return 1;
    return 0;
}
