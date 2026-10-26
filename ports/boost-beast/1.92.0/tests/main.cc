// Exercises Boost.Beast without the network: an HTTP request is
// serialized into a flat_buffer and parsed back, and a chunked response
// is parsed from memory, both through Beast's incremental parsers.
#include <boost/asio/buffer.hpp>
#include <boost/beast/core.hpp>
#include <boost/beast/http.hpp>
#include <string>

namespace beast = boost::beast;
namespace http = beast::http;

template <class Parser>
bool parse(Parser& parser, const std::string& wire) {
    parser.eager(true);
    boost::asio::const_buffer rest(wire.data(), wire.size());
    beast::error_code ec;
    while (!ec && !parser.is_done() && rest.size() != 0) rest += parser.put(rest, ec);
    return !ec && parser.is_done();
}

int main() {
    http::request<http::string_body> req{http::verb::post, "/items?id=7", 11};
    req.set(http::field::host, "example.com");
    req.body() = "hello";
    req.prepare_payload();

    beast::flat_buffer buffer;
    beast::ostream(buffer) << req;
    const std::string wire = beast::buffers_to_string(buffer.data());
    if (wire.compare(0, 27, "POST /items?id=7 HTTP/1.1\r\n") != 0) return 1;
    if (wire.find("Content-Length: 5\r\n") == std::string::npos) return 2;

    http::request_parser<http::string_body> request;
    if (!parse(request, wire)) return 3;
    const auto& got = request.get();
    if (got.method() != http::verb::post || got.target() != "/items?id=7" ||
        got[http::field::host] != "example.com" || got.body() != "hello")
        return 4;

    http::response_parser<http::string_body> response;
    if (!parse(response, "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n"
                         "5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n"))
        return 5;
    return response.get().result() == http::status::ok && response.get().body() == "hello world" ? 0 : 6;
}
