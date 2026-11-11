// The package's build script, in C++.  It links `tablegen`, the library
// of the package's build dependency, and writes the table that library
// computes as a C source, which the `app` target then compiles.  Only
// the script is C++: `app` itself stays a C program.

#include <cstdlib>
#include <fstream>
#include <string>

#include <tablegen/crc32.hpp>

int main() {
    const std::string out_dir = std::getenv("CABIN_OUT_DIR");
    const std::string source = out_dir + "/crc32_table.c";

    std::ofstream table(source);
    table << "const unsigned long crc32_table[256] = {\n";
    for (const auto entry : tablegen::crc32_table()) {
        table << "    " << entry << "UL,\n";
    }
    table << "};\n";
    table.close();
    if (!table) {
        return 1;
    }

    std::ofstream result(std::getenv("CABIN_RESULT_FILE"));
    result << "source app " << source << "\n";
    result.close();
    return result ? 0 : 1;
}
