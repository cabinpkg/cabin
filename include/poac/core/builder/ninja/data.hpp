#ifndef POAC_CORE_BUILDER_NINJA_DATA_HPP
#define POAC_CORE_BUILDER_NINJA_DATA_HPP

// std
#include <cstdint> // std::int64_t
#include <filesystem> // std::filesystem::path

// external
#include <ninja/build.h> // BuildConfig
#include <ninja/build_log.h> // BuildLog, BuildLogUser
#include <ninja/deps_log.h> // DepsLog
#include <ninja/disk_interface.h> // RealDiskInterface
#include <ninja/graph.h> // Node
#include <ninja/metrics.h> // GetTimeMillis
#include <ninja/state.h> // State
#include <ninja/string_piece.h> // StringPiece
#include <ninja/timestamp.h> // TimeStamp
#include <spdlog/spdlog.h> // spdlog::error

namespace poac::core::builder::ninja::data {
    struct NinjaMain: public BuildLogUser {
        NinjaMain(const BuildConfig& config, const std::filesystem::path& build_dir)
            : config(config), build_dir(build_dir) {}

        /// Build configuration set from flags (e.g. parallelism).
        const BuildConfig& config;

        /// Loaded state (rules, nodes).
        State state;

        /// Functions for accessing the disk.
        RealDiskInterface disk_interface;

        /// The build directory, used for storing the build log etc.
        std::filesystem::path build_dir;

        BuildLog build_log;
        DepsLog deps_log;

        std::int64_t start_time_millis = GetTimeMillis();

        virtual bool IsPathDead(StringPiece s) const {
            Node* n = state.LookupNode(s);
            if (n && n->in_edge()) {
                return false;
            }
            // Just checking n isn't enough: If an old output is both in the build log
            // and in the deps log, it will have a Node object in state_.  (It will also
            // have an in edge if one of its inputs is another output that's in the deps
            // log, but having a deps edge product an output that's input to another deps
            // edge is rare, and the first recompaction will delete all old outputs from
            // the deps log, and then a second recompaction will clear the build log,
            // which seems good enough for this corner case.)
            // Do keep entries around for files which still exist on disk, for
            // generators that want to use this information.
            std::string err;
            TimeStamp mtime = disk_interface.Stat(s.AsString(), &err);
            if (mtime == -1) {
                spdlog::error(err); // Log and ignore Stat() errors.
            }
            return mtime == 0;
        }
    };
} // end namespace

#endif // !POAC_CORE_BUILDER_NINJA_DATA_HPP
