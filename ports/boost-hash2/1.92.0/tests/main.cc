// Checks Boost.Hash2 algorithms against published test vectors (FNV-1a, MD5,
// SHA-256) and hashes a standard container through hash_append.
#include <boost/hash2/fnv1a.hpp>
#include <boost/hash2/hash_append.hpp>
#include <boost/hash2/md5.hpp>
#include <boost/hash2/sha2.hpp>
#include <cstdint>
#include <string>
#include <vector>

int main() {
    namespace h2 = boost::hash2;

    h2::fnv1a_32 fnv;
    fnv.update("foobar", 6);
    if (fnv.result() != 0xBF9CF968u) return 1;

    h2::md5_128 md5;
    md5.update("abc", 3);
    if (to_string(md5.result()) != "900150983cd24fb0d6963f7d28e17f72") return 1;

    h2::sha2_256 sha;
    sha.update("abc", 3);
    if (to_string(sha.result()) !=
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad")
        return 1;

    // hash_append of equal containers is deterministic; differing ones differ.
    std::vector<int> a(3, 7), b(3, 7), c(3, 8);
    h2::fnv1a_64 ha, hb, hc;
    h2::hash_append(ha, {}, a);
    h2::hash_append(hb, {}, b);
    h2::hash_append(hc, {}, c);
    std::uint64_t ra = ha.result();
    if (ra != hb.result() || ra == hc.result()) return 1;
    return 0;
}
