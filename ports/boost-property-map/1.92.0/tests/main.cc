// Exercises Boost.PropertyMap: get/put through associative, iterator,
// vector, function and composed property maps, and dynamic_properties
// converting values to and from strings by property name.
#include <boost/property_map/compose_property_map.hpp>
#include <boost/property_map/dynamic_property_map.hpp>
#include <boost/property_map/function_property_map.hpp>
#include <boost/property_map/property_map.hpp>
#include <boost/property_map/vector_property_map.hpp>

#include <cstddef>
#include <map>
#include <string>
#include <vector>

struct twice {
    int operator()(int x) const { return 2 * x; }
};

int main() {
    std::map<std::string, int> ages;
    boost::associative_property_map<std::map<std::string, int> > age(ages);
    boost::put(age, std::string("ada"), 36);
    if (boost::get(age, std::string("ada")) != 36 || ages.size() != 1) return 1;

    std::vector<double> weights(3, 0.0);
    boost::iterator_property_map<std::vector<double>::iterator, boost::identity_property_map> weight(
        weights.begin());
    boost::put(weight, std::size_t(2), 1.5);
    if (weights[2] != 1.5 || boost::get(weight, std::size_t(2)) != 1.5) return 1;

    boost::vector_property_map<int> grows;
    boost::put(grows, 10, 7);
    if (grows[10] != 7 || grows.storage_end() - grows.storage_begin() < 11) return 1;

    const boost::function_property_map<twice, int> doubled = boost::make_function_property_map<int>(twice());
    if (boost::get(doubled, 21) != 42) return 1;
    // compose_property_map's get is a hidden friend, found only by ADL.
    if (get(boost::make_compose_property_map(doubled, grows), std::size_t(10)) != 14) return 1;

    boost::dynamic_properties dp;
    dp.property("age", age);
    dp.property("weight", weight);
    if (!boost::put("age", dp, std::string("grace"), std::string("85")) || ages["grace"] != 85) return 1;
    if (boost::get("weight", dp, std::size_t(2)) != "1.5") return 1;
    try {
        boost::get("height", dp, std::size_t(0));
        return 1;
    } catch (const boost::dynamic_get_failure&) {
    }
    return boost::get<int>("age", dp, std::string("ada")) == 36 ? 0 : 1;
}
