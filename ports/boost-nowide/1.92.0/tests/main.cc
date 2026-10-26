// Exercises Boost.Nowide: UTF-8 argv, UTF-8/UTF-16 conversion, the
// environment wrappers, and a file with a non-ASCII name in the temp
// directory written, read back, stat'ed and removed.  On Windows each of
// these runs nowide's compiled wide-API wrappers.
#include <boost/nowide/args.hpp>
#include <boost/nowide/convert.hpp>
#include <boost/nowide/cstdio.hpp>
#include <boost/nowide/cstdlib.hpp>
#include <boost/nowide/fstream.hpp>
#include <boost/nowide/stat.hpp>

#include <ctime>
#include <sstream>
#include <string>

namespace nw = boost::nowide;

int main(int argc, char** argv) {
    nw::args utf8_args(argc, argv);
    if (argc < 1 || !argv[0] || !*argv[0]) return 1;

    const std::string word = "gr\xC3\xBC\xC3\x9F" "e-\xE2\x82\xAC";  // "grüße-€"
    const std::wstring wide = nw::widen(word);
    if (wide.size() != 7 || wide[2] != L'ü' || nw::narrow(wide) != word) return 1;

    if (nw::setenv("CABIN_NOWIDE_TEST", word.c_str(), 1) != 0) return 1;
    const char* value = nw::getenv("CABIN_NOWIDE_TEST");
    if (!value || word != value) return 1;
    if (nw::unsetenv("CABIN_NOWIDE_TEST") != 0 || nw::getenv("CABIN_NOWIDE_TEST")) return 1;

    const char* tmp = nw::getenv("TMPDIR");
    if (!tmp) tmp = nw::getenv("TEMP");  // Windows
    std::ostringstream name;
    name << (tmp ? tmp : "/tmp") << "/cabin-nowide-" << word << '-' << std::time(0) << ".txt";
    const std::string path = name.str();
    {
        nw::ofstream out(path.c_str(), std::ios::binary);
        out << word << '\n';
        if (!out) return 1;
    }
    std::string line;
    {
        nw::ifstream in(path.c_str(), std::ios::binary);
        std::getline(in, line);
    }
    nw::stat_t st;
    const bool same = nw::stat(path.c_str(), &st) == 0 &&
                      static_cast<unsigned long long>(st.st_size) == word.size() + 1 && line == word;
    return nw::remove(path.c_str()) == 0 && same ? 0 : 1;
}
