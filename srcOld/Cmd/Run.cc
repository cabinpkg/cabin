#include "Run.hpp"

// external
#include <spdlog/spdlog.h> // NOLINT(build/include_order)
#include <structopt/app.hpp>
#include <toml.hpp>

// internal
#include "../Data/Manifest.hpp"
#include "../Util/Format.hpp"
#include "../Util/Log.hpp"
#include "../Util/ResultMacros.hpp"
#include "../Util/Shell.hpp"
#include "../Util/Validator.hpp"
#include "./Build.hpp"

namespace poac::cmd::run {

[[nodiscard]] auto exec(const Options& opts) -> Result<void> {
  spdlog::trace("Checking if required config exists ...");
  Try(util::validator::required_config_exists().map_err(to_anyhow));

  spdlog::trace("Parsing the manifest file ...");
  // TODO(ken-matsui): parse as a static type rather than toml::value
  const toml::value manifest = toml::parse(data::manifest::NAME);
  const String name = toml::find<String>(manifest, "package", "name");

  const Option<Path> output = Try(
      build::build({.release = opts.release, .profile = opts.profile}, manifest)
          .with_context([&name] {
            return Err<build::FailedToBuild>(name).get();
          })
  );
  if (!output.has_value()) {
    return Ok();
  }

  const Path executable = output.value() / name;
  log::status("Running", executable);
  if (const i32 code = util::shell::Cmd(executable).exec_no_capture();
      code != 0) {
    return Err<SubprocessFailed>(executable, code);
  }
  return Ok();
}

} // namespace poac::cmd::run