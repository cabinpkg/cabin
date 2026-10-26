// Exercises Boost.URL's compiled parsers and formatters: URI parsing,
// component access, percent-decoding, modification, normalization,
// IP address parsing, and format().
#include <boost/url.hpp>
#include <string>

namespace urls = boost::urls;

int main() {
    auto parsed = urls::parse_uri("https://user@example.com:8080/a%20b/c?x=1&y=two#frag");
    if (!parsed) return 1;
    urls::url_view uv = *parsed;
    if (uv.scheme() != "https" || uv.host() != "example.com" || uv.port_number() != 8080) return 1;
    if (uv.path() != "/a b/c" || uv.encoded_path() != "/a%20b/c" || uv.fragment() != "frag") return 1;
    auto params = uv.params();
    if (params.size() != 2 || (*params.find("y")).value != "two") return 1;

    urls::url u = uv;
    u.set_host("cabinpkg.com");
    u.params().set("x", "a b");
    u.remove_fragment();
    if (u.buffer() != "https://user@cabinpkg.com:8080/a%20b/c?x=a%20b&y=two") return 1;

    urls::url messy("HTTP://Example.COM/a/./b/../c");
    messy.normalize();
    if (messy.buffer() != "http://example.com/a/c") return 1;

    if (urls::parse_uri("no scheme here")) return 1;
    auto ip = urls::parse_ipv4_address("192.168.0.1");
    if (!ip || ip->to_uint() != 0xC0A80001u) return 1;
    if (!urls::parse_ipv6_address("::1")) return 1;

    return urls::format("{}://{}/{}", "https", "example.org", "a b").buffer() ==
                   "https://example.org/a%20b"
               ? 0
               : 1;
}
