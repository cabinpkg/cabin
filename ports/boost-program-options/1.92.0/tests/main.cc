// Exercises Boost.Program_options' compiled parsers: a command line with
// typed, defaulted, composing and positional options, a config file read
// from a stream, shell-style splitting, an unknown-option error, and the
// generated help text.
#include <boost/program_options.hpp>

#include <sstream>
#include <string>
#include <vector>

namespace po = boost::program_options;
typedef std::vector<std::string> strings;

int main() {
    int level = 0;
    po::options_description desc("Options");
    desc.add_options()
        ("help,h", "show help")
        ("level,l", po::value<int>(&level)->default_value(1), "optimization level")
        ("define,D", po::value<strings>()->composing(), "macro")
        ("name", po::value<std::string>(), "package name")
        ("input", po::value<strings>(), "inputs");
    po::positional_options_description pos;
    pos.add("input", -1);

    const char* argv[] = {"prog", "-l", "3", "-DFOO", "--define=BAR", "a.cc", "b.cc"};
    po::variables_map vm;
    po::store(po::command_line_parser(7, argv).options(desc).positional(pos).run(), vm);
    std::istringstream config("# settings\nname = cabin\ndefine = BAZ\n");
    po::store(po::parse_config_file(config, desc), vm);
    po::notify(vm);

    if (level != 3 || vm.count("help")) return 1;
    const strings& defs = vm["define"].as<strings>();
    if (defs.size() != 3 || defs[0] != "FOO" || defs[1] != "BAR" || defs[2] != "BAZ") return 1;
    if (vm["name"].as<std::string>() != "cabin") return 1;
    if (vm["input"].as<strings>() != strings{"a.cc", "b.cc"}) return 1;

    const strings words = po::split_unix("--name 'two words' x");
    if (words.size() != 3 || words[1] != "two words") return 1;

    try {
        const char* bad[] = {"prog", "--no-such-option"};
        po::variables_map ignored;
        po::store(po::parse_command_line(2, bad, desc), ignored);
        return 1;
    } catch (const po::unknown_option& e) {
        if (std::string(e.what()).find("no-such-option") == std::string::npos) return 1;
    }

    std::ostringstream help;
    help << desc;
    return help.str().find("[ --level ] arg (=1)") != std::string::npos ? 0 : 1;
}
