// Exercises Boost.Tokenizer: char_separator with dropped/kept delimiters, escaped_list_separator
// for quoted CSV fields, and offset_separator for fixed-width fields.
#include <boost/tokenizer.hpp>
#include <string>
#include <vector>

int main() {
    typedef boost::tokenizer<boost::char_separator<char> > char_tok;
    const std::string words = "a,b;;c";
    char_tok t1(words, boost::char_separator<char>(",;"));
    std::vector<std::string> v(t1.begin(), t1.end());
    if (v.size() != 3 || v[0] != "a" || v[2] != "c") return 1;

    const std::string expr = "1+22";
    char_tok t2(expr, boost::char_separator<char>("", "+"));
    std::vector<std::string> e(t2.begin(), t2.end());
    if (e.size() != 3 || e[1] != "+" || e[2] != "22") return 1;

    const std::string csv = "x,\"y,z\",w";
    boost::tokenizer<boost::escaped_list_separator<char> > t3(csv);
    std::vector<std::string> c(t3.begin(), t3.end());
    if (c.size() != 3 || c[1] != "y,z") return 1;

    const int widths[] = {4, 2, 2};
    const std::string date = "20261026";
    boost::tokenizer<boost::offset_separator> t4(date, boost::offset_separator(widths, widths + 3));
    std::vector<std::string> d(t4.begin(), t4.end());
    if (d.size() != 3 || d[0] != "2026" || d[2] != "26") return 1;
    return 0;
}
