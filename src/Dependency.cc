#include "Dependency.hpp"

#include "Algos.hpp"
#include "Builder/Compiler.hpp"
#include "Command.hpp"
#include "Diag.hpp"
#include "Git2.hpp"
#include "Rustify/Result.hpp"

#include <cstdlib>
#include <filesystem>
#include <spdlog/spdlog.h>
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
static const fs::path CONAN_DIR(CACHE_DIR / "conan");
static const fs::path GIT_DIR(CACHE_DIR / "git");
static const fs::path GIT_SRC_DIR(GIT_DIR / "src");

Result<CompilerOpts>
GitDependency::install() const {
  fs::path installDir = GIT_SRC_DIR / name;
  if (target.has_value()) {
    installDir += '-' + target.value();
  }

  if (fs::exists(installDir) && !fs::is_empty(installDir)) {
    spdlog::debug("{} is already installed", name);
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

    Diag::info(
        "Downloaded", "{} {}", name, target.has_value() ? target.value() : url
    );
  }

  const fs::path includeDir = installDir / "include";
  fs::path include;

  if (fs::exists(includeDir) && fs::is_directory(includeDir)
      && !fs::is_empty(includeDir)) {
    include = includeDir;
  } else {
    include = installDir;
  }

  return Ok(CompilerOpts(
      CFlags({}, { IncludeDir{ include } }, {}),
      // Currently, no libs are supported.
      LdFlags()
  ));
}

Result<CompilerOpts>
PathDependency::install() const {
  const fs::path installDir = fs::weakly_canonical(path);
  if (fs::exists(installDir) && !fs::is_empty(installDir)) {
    spdlog::debug("{} is already installed", name);
  } else {
    Bail("{} can't be accessible as directory", installDir.string());
  }

  const fs::path includeDir = installDir / "include";
  fs::path include;

  if (fs::exists(includeDir) && fs::is_directory(includeDir)
      && !fs::is_empty(includeDir)) {
    include = includeDir;
  } else {
    include = installDir;
  }

  return Ok(CompilerOpts(
      CFlags({}, { IncludeDir{ include } }, {}),
      // Currently, no libs are supported.
      LdFlags()
  ));
}

Result<CompilerOpts>
SystemDependency::install() const {
  return CompilerOpts::parsePkgConfig(versionReq, name);
}

Result<CompilerOpts>
ConanDependency::install() const {
  std::string requiredPackage = versionReq.toConanString(name);
  fs::path conanOut = CONAN_DIR / name;
  auto conanCommand = Command("conan")
                          .addArg("install")
                          .addArg("--requires")
                          .addArg(requiredPackage)
                          .addArg("--build=missing")
                          .addArg("--output-folder")
                          .addArg(conanOut.native())
                          .addArg("--generator")
                          .addArg("PkgConfigDeps")
                          .addArg("--remote")
                          .addArg(remote);

  ExitStatus conanResult = Try(execCmd(conanCommand));
  Ensure(
      conanResult.success(), "conan failed with code {}", conanResult.exitCode()
  );

  return CompilerOpts::parsePkgConfig(versionReq, name, conanOut);
}

}  // namespace cabin
