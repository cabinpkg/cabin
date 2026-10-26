// Exercises Boost.JSON's compiled parser, serializer, DOM and
// conversions, including the incremental stream_parser and error codes.
#include <boost/json.hpp>

#include <string>
#include <vector>

namespace json = boost::json;

int main() {
    const json::value v = json::parse(
        "{\"name\":\"cabin\",\"tags\":[\"c\",\"c++\"],\"n\":3,\"pi\":3.25,\"ok\":true,\"none\":null}");
    const json::object& o = v.as_object();
    if (o.at("name").as_string() != "cabin" || o.at("tags").as_array().size() != 2) return 1;
    if (o.at("n").as_int64() != 3 || o.at("pi").as_double() != 3.25) return 1;
    if (!o.at("ok").as_bool() || !o.at("none").is_null()) return 1;
    if (json::parse(json::serialize(v)) != v) return 1;

    const json::value built = {{"a", 1}, {"b", {true, nullptr, "x"}}};
    if (json::serialize(built) != "{\"a\":1,\"b\":[true,null,\"x\"]}") return 1;

    boost::system::error_code ec;
    json::parse("{\"a\":", ec);
    if (!ec) return 1;

    const std::vector<int> nums = {1, 2, 3};
    if (json::value_to<std::vector<int> >(json::value_from(nums)) != nums) return 1;

    json::stream_parser p;
    p.write("[1, 2,");
    p.write(" 3]");
    p.finish();
    const json::value arr = p.release();
    return arr.as_array().size() == 3 && arr.at(2) == 3 ? 0 : 1;
}
