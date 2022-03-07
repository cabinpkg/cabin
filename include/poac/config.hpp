#ifndef POAC_CONFIG_HPP
#define POAC_CONFIG_HPP

#ifndef POAC_VERSION
#  error "POAC_VERSION is not defined"
#endif

// std
#include <filesystem>

// internal
#include <poac/util/misc.hpp>

namespace poac {
    constexpr char const* ALGOLIA_APPLICATION_ID =
            "IOCVK5FECM";
    constexpr char const* ALGOLIA_SEARCH_ONLY_KEY =
            "9c0a76bacf692daa9e8eca2aaff4b2ab";
    constexpr char const* ALGOLIA_INDEX_NAME =
            "packages";
    constexpr char const* ALGOLIA_SEARCH_INDEX_API_HOST =
            "IOCVK5FECM-dsn.algolia.net";
    constexpr char const* ALGOLIA_SEARCH_INDEX_API =
            "/1/indexes/packages/query";
} // end namespace

namespace poac::config::path {
    inline const std::filesystem::path user = util::misc::expand_user().unwrap();
    inline const std::filesystem::path root(user / ".poac");
    inline const std::filesystem::path cache_dir(root / "cache");
    inline const std::filesystem::path archive_dir(cache_dir / "archive");
    inline const std::filesystem::path extract_dir(cache_dir / "extract");
} // end namespace

#endif // !POAC_CONFIG_HPP
