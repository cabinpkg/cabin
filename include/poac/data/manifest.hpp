#ifndef POAC_DATA_MANIFEST_HPP_
#define POAC_DATA_MANIFEST_HPP_

// internal
#include <poac/poac.hpp>

namespace poac::data::manifest {

inline const String name = "poac.toml";

inline fs::file_time_type
poac_toml_last_modified(const fs::path& base_dir) {
  return fs::last_write_time(base_dir / name);
}

} // namespace poac::data::manifest

#endif // POAC_DATA_MANIFEST_HPP_
