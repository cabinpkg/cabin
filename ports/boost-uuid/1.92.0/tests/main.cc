// Boost.UUID: name-based (v5, v3) generation against known vectors, string parsing, nil UUIDs,
// and string round trips.
#include <boost/uuid/uuid.hpp>
#include <boost/uuid/uuid_io.hpp>
#include <boost/uuid/name_generator_sha1.hpp>
#include <boost/uuid/name_generator_md5.hpp>
#include <boost/uuid/string_generator.hpp>
#include <boost/uuid/nil_generator.hpp>
#include <boost/uuid/namespaces.hpp>
#include <string>

int main() {
    using namespace boost::uuids;

    boost::uuids::uuid v5 = name_generator_sha1(ns::dns())("www.boost.org");
    if (to_string(v5) != "ccb046db-74eb-53c9-9b6b-3ce5740dd29e") return 1;
    if (v5.version() != boost::uuids::uuid::version_name_based_sha1) return 1;

    boost::uuids::uuid v3 = name_generator_md5(ns::dns())("www.boost.org");
    if (to_string(v3) != "1c279409-7f03-32bc-be93-35ddcdf48536") return 1;

    boost::uuids::uuid parsed = string_generator()("{ccb046db-74eb-53c9-9b6b-3ce5740dd29e}");
    if (parsed != v5 || parsed == v3) return 1;

    boost::uuids::uuid n = nil_uuid();
    if (!n.is_nil() || v5.is_nil() || to_string(n) != "00000000-0000-0000-0000-000000000000") return 1;
    return n < v5 ? 0 : 1;
}
