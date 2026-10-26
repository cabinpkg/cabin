// Exercises Boost.Log's compiled core, global logger, attributes and
// stream sink (boost_log), and its filter and formatter parsers
// (boost_log_setup): trivial-logger records pass a parsed severity
// filter into a sink that formats them with a parsed format string.
// An IPC object name exercises the platform's compiled IPC sources.
#include <boost/log/attributes/constant.hpp>
#include <boost/log/core/core.hpp>
#include <boost/log/sinks/sync_frontend.hpp>
#include <boost/log/sinks/text_ostream_backend.hpp>
#include <boost/log/trivial.hpp>
#include <boost/log/utility/ipc/object_name.hpp>
#include <boost/log/utility/setup/filter_parser.hpp>
#include <boost/log/utility/setup/formatter_parser.hpp>
#include <boost/smart_ptr/make_shared_object.hpp>
#include <sstream>
#include <string>

namespace logging = boost::log;
typedef logging::sinks::synchronous_sink<logging::sinks::text_ostream_backend> Sink;

int main() {
    const boost::shared_ptr<std::ostringstream> out = boost::make_shared<std::ostringstream>();
    const boost::shared_ptr<logging::sinks::text_ostream_backend> backend =
        boost::make_shared<logging::sinks::text_ostream_backend>();
    backend->add_stream(out);
    const boost::shared_ptr<Sink> sink = boost::make_shared<Sink>(backend);
    sink->set_filter(logging::parse_filter("%Severity% >= warning"));
    sink->set_formatter(logging::parse_formatter("%Severity%|%Tag%|%Message%"));

    const boost::shared_ptr<logging::core> core = logging::core::get();
    core->add_sink(sink);
    core->add_global_attribute("Tag", logging::attributes::constant<std::string>("cabin"));

    BOOST_LOG_TRIVIAL(info) << "dropped";
    BOOST_LOG_TRIVIAL(warning) << "kept " << 1;
    core->set_filter(logging::parse_filter("%Tag% != cabin"));
    BOOST_LOG_TRIVIAL(fatal) << "dropped by the core";
    core->reset_filter();
    BOOST_LOG_TRIVIAL(error) << "kept " << 2;
    sink->flush();
    core->remove_all_sinks();

    const std::string name =
        logging::ipc::object_name(logging::ipc::object_name::global, "cabin").c_str();
    const std::string suffix = "boost.log.global.cabin";
    if (name.size() < suffix.size() ||
        name.compare(name.size() - suffix.size(), suffix.size(), suffix) != 0) {
        return 1;
    }

    return out->str() == "warning|cabin|kept 1\nerror|cabin|kept 2\n" ? 0 : 1;
}
