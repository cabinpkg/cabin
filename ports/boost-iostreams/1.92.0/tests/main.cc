// Exercises Boost.Iostreams' compiled pieces: a gzip and a zlib round
// trip through the zlib-backed filters, and a temporary file written
// with file_descriptor_sink and read back through mapped_file_source.
#include <boost/iostreams/copy.hpp>
#include <boost/iostreams/device/array.hpp>
#include <boost/iostreams/device/back_inserter.hpp>
#include <boost/iostreams/device/file_descriptor.hpp>
#include <boost/iostreams/device/mapped_file.hpp>
#include <boost/iostreams/filter/gzip.hpp>
#include <boost/iostreams/filter/zlib.hpp>
#include <boost/iostreams/filtering_stream.hpp>
#include <boost/iostreams/write.hpp>

#include <cstdio>
#include <cstdlib>
#include <string>

namespace io = boost::iostreams;

template <class Compressor, class Decompressor>
static bool round_trip(const std::string& text, std::string& packed) {
    {
        io::filtering_ostream out;
        out.push(Compressor());
        out.push(io::back_inserter(packed));
        out << text;
    }
    std::string unpacked;
    io::filtering_istream in;
    in.push(Decompressor());
    in.push(io::array_source(packed.data(), packed.size()));
    io::copy(in, io::back_inserter(unpacked));
    return unpacked == text && packed.size() < text.size();
}

int main() {
    std::string text;
    for (int i = 0; i < 200; ++i) text += "Cabin builds Boost.Iostreams. ";

    std::string gz, z;
    if (!round_trip<io::gzip_compressor, io::gzip_decompressor>(text, gz)) return 1;
    if (static_cast<unsigned char>(gz[0]) != 0x1f || static_cast<unsigned char>(gz[1]) != 0x8b) return 1;
    if (!round_trip<io::zlib_compressor, io::zlib_decompressor>(text, z)) return 1;

    const char* tmp = std::getenv("TMPDIR");
    if (!tmp) tmp = std::getenv("TEMP");  // Windows
    const std::string name = std::string(tmp ? tmp : "/tmp") + "/cabin-iostreams-test.tmp";
    {
        io::file_descriptor_sink sink(name, std::ios_base::out | std::ios_base::trunc);
        io::write(sink, text.data(), static_cast<std::streamsize>(text.size()));
    }
    bool same;
    {
        io::mapped_file_source map(name);
        same = map.size() == text.size() && std::string(map.data(), map.size()) == text;
    }
    std::remove(name.c_str());
    return same ? 0 : 1;
}
