// Exercises Boost.IO stream state savers restoring flags and fill, and
// boost::io::quoted output and round-trip input.
#include <boost/io/ios_state.hpp>
#include <boost/io/quoted.hpp>
#include <sstream>
#include <string>

int main() {
    std::ostringstream os;
    {
        boost::io::ios_all_saver saver(os);
        os.setf(std::ios_base::hex, std::ios_base::basefield);
        os.fill('*');
        os.width(4);
        os << 255;
    }
    os << ' ' << 255;
    if (os.str() != "**ff 255" || os.fill() != ' ') return 1;

    std::ostringstream qs;
    qs << boost::io::quoted(std::string("say \"hi\""));
    if (qs.str() != "\"say \\\"hi\\\"\"") return 1;

    std::istringstream is(qs.str());
    std::string back;
    is >> boost::io::quoted(back);
    if (back != "say \"hi\"") return 1;
    return 0;
}
