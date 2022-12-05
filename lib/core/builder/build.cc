// std
#include <cstdlib> // setenv

// external
#include <ninja/build.h> // NOLINT(build/include_order)
#include <ninja/graph.h> // NOLINT(build/include_order)
#include <ninja/manifest_parser.h> // NOLINT(build/include_order)
#include <ninja/status.h> // StatusPrinter // NOLINT(build/include_order)
#include <spdlog/spdlog.h> // NOLINT(build/include_order)

// internal
#include "poac/config.hpp"
#include "poac/core/builder/build.hpp"
#include "poac/core/builder/log.hpp"
#include "poac/core/builder/manifest.hpp"
#include "poac/util/verbosity.hpp"

namespace poac::core::builder::build {

auto to_string(Mode mode) -> String {
  switch (mode) {
    case Mode::debug:
      return "debug";
    case Mode::release:
      return "release";
    default:
      unreachable();
  }
}

auto operator<<(std::ostream& os, Mode mode) -> std::ostream& {
  switch (mode) {
    case Mode::debug:
      return (os << "dev");
    case Mode::release:
      return (os << "release");
    default:
      unreachable();
  }
}

/// Build the targets listed on the command line.
[[nodiscard]] auto run(data::NinjaMain& ninja_main, Status& status)
    -> Result<void> {
  String err;
  Vec<Node*> targets = ninja_main.state.DefaultNodes(&err);
  if (!err.empty()) {
    return Err<GeneralError>(err);
  }
  ninja_main.disk_interface.AllowStatCache(true);

  Builder builder(
      &ninja_main.state, ninja_main.config, &ninja_main.build_log,
      &ninja_main.deps_log, &ninja_main.disk_interface, &status,
      ninja_main.start_time_millis
  );
  for (Node* target : targets) {
    if (!builder.AddTarget(target, &err)) {
      if (!err.empty()) {
        return Err<GeneralError>(err);
      }
      // Added a target that is already up-to-date; not really an error.
    }
  }
  // Make sure restat rules do not see stale timestamps.
  ninja_main.disk_interface.AllowStatCache(false);

  if (builder.AlreadyUpToDate()) {
    spdlog::trace("nothing to do.");
    return Ok();
  }
  if (!builder.Build(&err)) {
    return Err<GeneralError>(err);
  }
  return Ok();
}

auto get_ninja_verbosity() -> BuildConfig::Verbosity {
  if (util::verbosity::is_verbose()) {
    return BuildConfig::VERBOSE;
  } else if (util::verbosity::is_quiet()) {
    return BuildConfig::QUIET;
  } else {
    return BuildConfig::NORMAL;
  }
}

[[nodiscard]] auto start(
    const toml::value& poac_manifest, const Mode& mode,
    const resolver::ResolvedDeps& resolved_deps
) -> Result<Path> {
  BuildConfig config;

  // ref: https://github.com/ninja-build/ninja/pull/2102#issuecomment-1147771497
  setenv("TERM", "dumb", true);
  const String progress_status_format =
      termcolor2::should_color()
          ? format("{:>27} %f/%t: ", "Compiling"_bold_green)
          : format("{:>12} %f/%t: ", "Compiling");
  setenv("NINJA_STATUS", progress_status_format.c_str(), true);
  StatusPrinter status(config);

  const Path build_dir = config::path::out_dir / to_string(mode);
  fs::create_directories(build_dir);
  Try(manifest::create(build_dir, poac_manifest, resolved_deps));

  for (i32 cycle = 1; cycle <= rebuildLimit; ++cycle) {
    data::NinjaMain ninja_main(config, build_dir);
    ManifestParserOptions parser_opts;
    parser_opts.dupe_edge_action_ = kDupeEdgeActionError;
    ManifestParser parser(
        &ninja_main.state, &ninja_main.disk_interface, parser_opts
    );
    String err;
    if (!parser.Load(
            (ninja_main.build_dir / manifest::manifest_file_name).string(), &err
        )) {
      return Err<GeneralError>(err);
    }

    Try(log::load_build_log(ninja_main));
    Try(log::load_deps_log(ninja_main));

    // Attempt to rebuild the manifest before building anything else
    if (manifest::rebuild(ninja_main, status, err)) {
      // Start the build over with the new manifest.
      continue;
    } else if (!err.empty()) {
      return Err<GeneralError>(err);
    }

    Try(run(ninja_main, status));
    return Ok(config::path::out_dir / to_string(mode));
  }
  return Err<GeneralError>(format(
      "internal manifest still dirty after {} tries, perhaps system time is not set",
      rebuildLimit
  ));
}

} // namespace poac::core::builder::build
