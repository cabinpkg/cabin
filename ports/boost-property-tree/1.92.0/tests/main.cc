// Exercises Boost.PropertyTree: a tree built through dotted paths with
// typed get/put and defaults is written to and read back from JSON,
// INI, XML and INFO in memory, and malformed JSON raises a parser error.
#include <boost/property_tree/info_parser.hpp>
#include <boost/property_tree/ini_parser.hpp>
#include <boost/property_tree/json_parser.hpp>
#include <boost/property_tree/ptree.hpp>
#include <boost/property_tree/xml_parser.hpp>
#include <sstream>
#include <string>

namespace pt = boost::property_tree;

template <class Write, class Read> static bool round_trip(const pt::ptree& tree, Write write, Read read) {
    std::stringstream stream;
    write(stream, tree);
    pt::ptree back;
    read(stream, back);
    return back.get<std::string>("server.host") == "example.com" && back.get<int>("server.port") == 8080 &&
           back.get<double>("limits.ratio") == 0.25 && back.get<bool>("limits.strict");
}

int main() {
    pt::ptree tree;
    tree.put("server.host", "example.com");
    tree.put("server.port", 8080);
    tree.put("limits.ratio", 0.25);
    tree.put("limits.strict", true);
    if (tree.get("server.missing", 7) != 7 || tree.get_optional<int>("limits.none")) return 1;
    if (tree.get_child("server").size() != 2) return 2;

    if (!round_trip(tree, [](std::ostream& s, const pt::ptree& t) { pt::write_json(s, t); },
                    [](std::istream& s, pt::ptree& t) { pt::read_json(s, t); }))
        return 3;
    if (!round_trip(tree, [](std::ostream& s, const pt::ptree& t) { pt::write_ini(s, t); },
                    [](std::istream& s, pt::ptree& t) { pt::read_ini(s, t); }))
        return 4;
    if (!round_trip(tree, [](std::ostream& s, const pt::ptree& t) { pt::write_xml(s, t); },
                    [](std::istream& s, pt::ptree& t) { pt::read_xml(s, t); }))
        return 5;
    if (!round_trip(tree, [](std::ostream& s, const pt::ptree& t) { pt::write_info(s, t); },
                    [](std::istream& s, pt::ptree& t) { pt::read_info(s, t); }))
        return 6;

    std::istringstream bad("{\"a\": [1, 2");
    pt::ptree ignored;
    try {
        pt::read_json(bad, ignored);
    } catch (const pt::json_parser_error&) {
        return 0;
    }
    return 7;
}
