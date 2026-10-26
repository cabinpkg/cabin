// Boost.Unordered: insert, find, count, and erase in unordered_map, unordered_flat_map,
// and unordered_set.
#include <boost/unordered_map.hpp>
#include <boost/unordered_set.hpp>
#include <boost/unordered/unordered_flat_map.hpp>
#include <string>

int main() {
    boost::unordered_map<std::string, int> m;
    m["one"] = 1;
    m.insert(std::make_pair(std::string("two"), 2));
    if (m.size() != 2 || m.find("two") == m.end() || m.at("one") != 1) return 1;
    if (m.erase("one") != 1 || m.count("one") != 0) return 1;

    boost::unordered_flat_map<int, std::string> fm;
    for (int i = 0; i < 100; ++i) fm.emplace(i, std::to_string(i));
    if (fm.size() != 100 || fm[42] != "42") return 1;
    if (fm.insert({5, "dup"}).second) return 1;
    if (fm.at(5) != "5") return 1;
    fm.erase(42);
    if (fm.contains(42) || !fm.contains(41)) return 1;

    boost::unordered_set<std::string> s;
    s.insert("a");
    s.insert("b");
    s.insert("a");
    return (s.size() == 2 && s.count("b") == 1 && s.count("c") == 0) ? 0 : 1;
}
