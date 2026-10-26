// Exercises Boost.MQTT5 without a broker: a PUBLISH packet with
// properties is encoded and decoded back through the MQTT v5 codecs,
// and a publish queued on an mqtt_client that never runs (so it never
// connects) must complete with operation_aborted once cancel() is called.
#include <boost/asio/io_context.hpp>
#include <boost/asio/ip/tcp.hpp>
#include <boost/mqtt5.hpp>
#include <boost/mqtt5/impl/codecs/message_decoders.hpp>
#include <boost/mqtt5/impl/codecs/message_encoders.hpp>
#include <string>

namespace asio = boost::asio;
namespace mqtt5 = boost::mqtt5;

static bool codec_round_trip() {
    mqtt5::publish_props props;
    props[mqtt5::prop::message_expiry_interval] = 70;
    props[mqtt5::prop::content_type] = std::string("text/plain");
    props[mqtt5::prop::user_property].emplace_back("key", "val");
    const std::string wire = mqtt5::encoders::encode_publish(
        31283, "sensors/temp", "21.5", mqtt5::qos_e::at_least_once, mqtt5::retain_e::yes, mqtt5::dup_e::no, props);

    mqtt5::decoders::byte_citer it = wire.cbegin();
    const auto header = mqtt5::decoders::decode_fixed_header(it, wire.cend());
    if (!header) return false;
    const auto [control_byte, remain_length] = *header;
    const auto msg = mqtt5::decoders::decode_publish(control_byte, remain_length, it);
    if (!msg) return false;
    const auto& [topic, packet_id, flags, got, payload] = *msg;
    return topic == "sensors/temp" && packet_id && *packet_id == 31283 && payload == "21.5" && flags == 0b0011 &&
           *got[mqtt5::prop::message_expiry_interval] == 70u && *got[mqtt5::prop::content_type] == "text/plain" &&
           got[mqtt5::prop::user_property].size() == 1 && got[mqtt5::prop::user_property][0].second == "val";
}

int main() {
    if (!codec_round_trip()) return 1;

    asio::io_context ioc;
    mqtt5::mqtt_client<asio::ip::tcp::socket> client(ioc);
    client.credentials("cabin-probe");
    int aborted = 0;
    client.async_publish<mqtt5::qos_e::at_most_once>(
        "sensors/temp", "21.5", mqtt5::retain_e::no, mqtt5::publish_props{},
        [&](mqtt5::error_code ec) { aborted += ec == asio::error::operation_aborted; });
    client.cancel();
    ioc.run();
    return aborted == 1 ? 0 : 2;
}
