// Exercises Boost.Parser: number lists with a whitespace skipper (the
// floating-point path goes through std or Boost.Charconv from_chars),
// a failed parse and its diagnostic, and semantic actions.
#include <boost/parser/parser.hpp>

#include <optional>
#include <sstream>
#include <string>
#include <vector>

namespace bp = boost::parser;

int main() {
    const auto csv = bp::double_ % ',';
    const std::optional<std::vector<double>> nums = bp::parse("1.5, -2, 3e2", csv, bp::ws);
    if (!nums || nums->size() != 3) return 1;
    if ((*nums)[0] != 1.5 || (*nums)[1] != -2.0 || (*nums)[2] != 300.0) return 1;
    std::ostringstream diagnostics;
    bp::stream_error_handler errors("", diagnostics);
    if (bp::parse("1.5, x", bp::with_error_handler(csv, errors), bp::ws)) return 1;
    if (diagnostics.str().empty()) return 1;

    const auto list = bp::parse("[10,20,30]", '[' >> (bp::int_ % ',') >> ']');
    if (!list || *list != std::vector<int>{10, 20, 30}) return 1;

    int total = 0;
    const auto add = [&total](auto& ctx) { total += bp::_attr(ctx); };
    if (!bp::parse("1 2 3 4", *bp::int_[add], bp::ws) || total != 10) return 1;

    const std::optional<std::string> word = bp::parse("cabin!", +bp::char_('a', 'z') >> '!');
    return word && *word == "cabin" ? 0 : 1;
}
