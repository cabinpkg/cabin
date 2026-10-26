// Exercises Boost.Serialization's compiled archives: a derived object
// holding standard containers round-trips through a base-class pointer
// (exported type info and void_cast) in the text, binary and XML
// archives of boost_serialization and the wide text and XML archives
// of boost_wserialization.
#include <boost/archive/binary_iarchive.hpp>
#include <boost/archive/binary_oarchive.hpp>
#include <boost/archive/text_iarchive.hpp>
#include <boost/archive/text_oarchive.hpp>
#include <boost/archive/text_wiarchive.hpp>
#include <boost/archive/text_woarchive.hpp>
#include <boost/archive/xml_iarchive.hpp>
#include <boost/archive/xml_oarchive.hpp>
#include <boost/archive/xml_wiarchive.hpp>
#include <boost/archive/xml_woarchive.hpp>
#include <boost/serialization/base_object.hpp>
#include <boost/serialization/export.hpp>
#include <boost/serialization/map.hpp>
#include <boost/serialization/string.hpp>
#include <boost/serialization/vector.hpp>
#include <sstream>

using boost::serialization::make_nvp;

struct Base {
    int id = 0;
    virtual ~Base() {}
    template <class Archive> void serialize(Archive& ar, unsigned) { ar & BOOST_SERIALIZATION_NVP(id); }
};

struct Item : Base {
    std::string name = "cabin <&> port";
    std::vector<int> values = {3, -1, 4, 1, 5};
    std::map<std::string, double> weights = {{"alpha", 0.5}, {"beta", -2.25}};
    template <class Archive> void serialize(Archive& ar, unsigned) {
        ar & make_nvp("base", boost::serialization::base_object<Base>(*this));
        ar & BOOST_SERIALIZATION_NVP(name) & BOOST_SERIALIZATION_NVP(values) & BOOST_SERIALIZATION_NVP(weights);
    }
};
BOOST_CLASS_EXPORT(Item)

template <class OArchive, class IArchive, class Stream> bool round_trip() {
    Item in;
    in.id = 42;
    const Base* saved = &in;
    Base* loaded = nullptr;
    Stream stream;
    {
        OArchive out(stream);
        out << make_nvp("item", saved);
    }
    IArchive is(stream);
    is >> make_nvp("item", loaded);
    const Item* got = dynamic_cast<const Item*>(loaded);
    const bool ok = got && got->id == 42 && got->name == in.name && got->values == in.values && got->weights == in.weights;
    delete loaded;
    return ok;
}

int main() {
    namespace ar = boost::archive;
    if (!round_trip<ar::text_oarchive, ar::text_iarchive, std::stringstream>()) return 1;
    if (!round_trip<ar::binary_oarchive, ar::binary_iarchive, std::stringstream>()) return 2;
    if (!round_trip<ar::xml_oarchive, ar::xml_iarchive, std::stringstream>()) return 3;
    if (!round_trip<ar::text_woarchive, ar::text_wiarchive, std::wstringstream>()) return 4;
    return round_trip<ar::xml_woarchive, ar::xml_wiarchive, std::wstringstream>() ? 0 : 5;
}
