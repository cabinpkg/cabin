#pragma once

#include "Algos.hpp"
#include "Command.hpp"
#include "Manifest.hpp"
#include "Rustify/Result.hpp"

#include <array>
#include <chrono>
#include <cstdint>
#include <cstdlib>
#include <filesystem>
#include <fmt/format.h>
#include <fstream>
#include <initializer_list>
#include <iterator>
#include <regex>
#include <sstream>
#include <string>
#include <string_view>
#include <unistd.h>
#include <utility>
#include <vector>

namespace tests {

namespace fs = std::filesystem;

inline std::string readFile(const fs::path& file);

inline const fs::path& projectRoot() {
  static const fs::path root = [] {
    auto manifest = cabin::Manifest::tryParse().unwrap();
    return manifest.path.parent_path();
  }();
  return root;
}

inline fs::path cabinBinary() {
  if (const char* env = std::getenv("CABIN")) {
    return fs::path(env);
  }
  const auto& root = projectRoot();
  const std::array<fs::path, 3> candidates = {
    root / "build" / "cabin",
    root / "cabin-out" / "dev" / "cabin",
  };
  for (const auto& candidate : candidates) {
    if (fs::exists(candidate)) {
      return candidate;
    }
  }
  return candidates.front();
}

struct RunResult {
  cabin::ExitStatus status;
  std::string out;
  std::string err;
};

inline std::string replaceAll(std::string text, std::string_view from,
                              std::string_view to) {
  if (from.empty()) {
    return text;
  }
  std::size_t pos = 0;
  while ((pos = text.find(from, pos)) != std::string::npos) {
    text.replace(pos, from.size(), to);
    pos += to.size();
  }
  return text;
}

inline std::string scrubDurations(std::string text) {
  static const std::regex pattern(R"(in [0-9]+\.[0-9]+s)");
  return std::regex_replace(text, pattern, "in <DURATION>s");
}

inline std::string scrubIsoDates(std::string text) {
  static const std::regex pattern(R"([0-9]{4}-[0-9]{2}-[0-9]{2})");
  return std::regex_replace(text, pattern, "<DATE>");
}

inline std::string sanitizeOutput(
    std::string text,
    std::initializer_list<std::pair<std::string_view, std::string_view>>
        replacements = {}) {
  for (const auto& [from, to] : replacements) {
    text = replaceAll(std::move(text), from, to);
  }
  text = scrubDurations(std::move(text));
  text = scrubIsoDates(std::move(text));
  text = std::regex_replace(std::move(text), std::regex(R"(\b[0-9a-f]{40}\b)"),
                            "<COMMIT_HASH>");
  text = std::regex_replace(std::move(text), std::regex(R"(\b[0-9a-f]{8}\b)"),
                            "<SHORT_HASH>");
  text = std::regex_replace(
      std::move(text),
      std::regex(R"(^compiler: .*$)", std::regex_constants::multiline),
      "compiler: <COMPILER>");
  text = std::regex_replace(
      std::move(text),
      std::regex(R"(^libgit2: .*$)", std::regex_constants::multiline),
      "libgit2: <LIBGIT2>");
  text = std::regex_replace(
      std::move(text),
      std::regex(R"(^libcurl: .*$)", std::regex_constants::multiline),
      "libcurl: <LIBCURL>");
  return text;
}

inline Result<RunResult> runCabin(const std::vector<std::string>& args,
                                  const fs::path& workdir = {}) {
  ::setenv("CABIN_TERM_COLOR", "never", 1);
  cabin::Command cmd(cabinBinary().string());
  for (const auto& arg : args) {
    cmd.addArg(arg);
  }
  if (!workdir.empty()) {
    cmd.setWorkingDirectory(workdir);
  }
  cmd.setStdOutConfig(cabin::Command::IOConfig::Piped);
  cmd.setStdErrConfig(cabin::Command::IOConfig::Piped);

  const cabin::CommandOutput output = Try(cmd.output());
  return Ok(RunResult{ output.exitStatus, output.stdOut, output.stdErr });
}

inline Result<RunResult> runCabin(std::initializer_list<std::string> args,
                                  const fs::path& workdir = {}) {
  return runCabin(std::vector<std::string>(args), workdir);
}

struct TempDir {
  fs::path path;

  TempDir()
      : path([] {
          const auto epoch =
              std::chrono::steady_clock::now().time_since_epoch();
          const auto ticks =
              std::chrono::duration_cast<std::chrono::nanoseconds>(epoch)
                  .count();
          std::ostringstream oss;
          oss << "cabin-test-" << static_cast<std::int64_t>(::getpid()) << '-'
              << ticks;
          return fs::temp_directory_path() / oss.str();
        }()) {
    fs::create_directories(path);
  }

  ~TempDir() {
    if (path.empty()) {
      return;
    }
    std::error_code ec;
    fs::remove_all(path, ec);
  }

  TempDir(const TempDir&) = delete;
  TempDir& operator=(const TempDir&) = delete;

  TempDir(TempDir&& other) noexcept : path(std::move(other.path)) {
    other.path.clear();
  }

  TempDir& operator=(TempDir&& other) noexcept {
    if (this != &other) {
      path = std::move(other.path);
      other.path.clear();
    }
    return *this;
  }

  [[nodiscard]] fs::path operator/(const fs::path& relative) const {
    return path / relative;
  }
};

inline std::string readFile(const fs::path& file) {
  std::ifstream ifs(file);
  return std::string(std::istreambuf_iterator<char>(ifs), {});
}

inline void writeFile(const fs::path& file, const std::string& content) {
  std::ofstream ofs(file);
  ofs << content;
}

inline bool hasCommand(std::string_view name) {
  return cabin::commandExists(name);
}

} // namespace tests
