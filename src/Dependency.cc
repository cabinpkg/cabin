#include "Dependency.hpp"

#include "Algos.hpp"
#include "Command.hpp"
#include "Git2.hpp"
#include "Logger.hpp"

#include <cstdlib>
#include <filesystem>
#include <string>

namespace cabin {

namespace fs = std::filesystem;

static fs::path
getXdgCacheHome() noexcept {
  if (const char* envP = std::getenv("XDG_CACHE_HOME")) {
    return envP;
  }
  const fs::path userDir = std::getenv("HOME");
  return userDir / ".cache";
}

static const fs::path CACHE_DIR(getXdgCacheHome() / "cabin");
static const fs::path GIT_DIR(CACHE_DIR / "git");
static const fs::path GIT_SRC_DIR(GIT_DIR / "src");

Result<DepMetadata>
GitDependency::install() const {
  fs::path installDir = GIT_SRC_DIR / name;
  if (target.has_value()) {
    installDir += '-' + target.value();
  }

  if (fs::exists(installDir) && !fs::is_empty(installDir)) {
    logger::debug("{} is already installed", name);
  } else {
    git2::Repository repo;
    repo.clone(url, installDir.string());

    if (target.has_value()) {
      // Checkout to target.
      const std::string target = this->target.value();
      const git2::Object obj = repo.revparseSingle(target);
      repo.setHeadDetached(obj.id());
      repo.checkoutHead(true);
    }

    logger::info(
        "Downloaded", "{} {}", name, target.has_value() ? target.value() : url
    );
  }

  const fs::path includeDir = installDir / "include";
  std::string includes = "-isystem";

  if (fs::exists(includeDir) && fs::is_directory(includeDir)
      && !fs::is_empty(includeDir)) {
    includes += includeDir.string();
  } else {
    includes += installDir.string();
  }

  // Currently, no libs are supported.
  return Ok(DepMetadata{ .includes = includes, .libs = "" });
}

Result<DepMetadata>
PathDependency::install() const {
  const fs::path installDir = fs::weakly_canonical(path);
  if (fs::exists(installDir) && !fs::is_empty(installDir)) {
    logger::debug("{} is already installed", name);
  } else {
    Bail("{} can't be accessible as directory", installDir.string());
  }

  const fs::path includeDir = installDir / "include";
  std::string includes = "-isystem";

  if (fs::exists(includeDir) && fs::is_directory(includeDir)
      && !fs::is_empty(includeDir)) {
    includes += includeDir.string();
  } else {
    includes += installDir.string();
  }

  // Currently, no libs are supported.
  return Ok(DepMetadata{ .includes = includes, .libs = "" });
}

Result<DepMetadata>
SystemDependency::install() const {
  const std::string pkgConfigVer = versionReq.toPkgConfigString(name);
  const Command cflagsCmd =
      Command("pkg-config").addArg("--cflags").addArg(pkgConfigVer);
  const Command libsCmd =
      Command("pkg-config").addArg("--libs").addArg(pkgConfigVer);

  std::string cflags = Try(getCmdOutput(cflagsCmd));
  cflags.pop_back();  // remove '\n'
  std::string libs = Try(getCmdOutput(libsCmd));
  libs.pop_back();  // remove '\n'

  return Ok(DepMetadata{ .includes = cflags, .libs = libs });
}

}  // namespace cabin
