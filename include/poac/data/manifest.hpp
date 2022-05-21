#ifndef POAC_DATA_MANIFEST_HPP
#define POAC_DATA_MANIFEST_HPP

// internal
#include <poac/poac.hpp>

namespace poac::data::manifest {
    inline const String manifest_file_name = "poac.toml";

    inline fs::file_time_type
    poac_toml_last_modified(const fs::path& base_dir) {
        return fs::last_write_time(base_dir / manifest_file_name);
    }
}

#endif // !POAC_DATA_MANIFEST_HPP
