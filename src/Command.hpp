#pragma once

#include "Rustify/Result.hpp"

#include <cstdint>
#include <cstdlib>
#include <filesystem>
#include <fmt/format.h>
#include <span>
#include <string>
#include <string_view>
#include <utility>
#include <vector>

namespace cabin {

class ExitStatus {
  int rawStatus; // Original status from waitpid

public:
  ExitStatus() noexcept : rawStatus(EXIT_SUCCESS) {}
  explicit ExitStatus(int status) noexcept : rawStatus(status) {}

  bool exitedNormally() const noexcept;
  bool killedBySignal() const noexcept;
  bool stoppedBySignal() const noexcept;
  int exitCode() const noexcept;
  int termSignal() const noexcept;
  int stopSignal() const noexcept;
  bool coreDumped() const noexcept;

  // Successful only if normally exited with code 0
  bool success() const noexcept;

  std::string toString() const;
};

struct CommandOutput {
  const ExitStatus exitStatus;
  const std::string stdOut;
  const std::string stdErr;
};

class Child {
private:
  const pid_t pid;
  const int stdOutFd;
  const int stdErrFd;

  Child(pid_t pid, int stdOutFd, int stdErrFd) noexcept
      : pid(pid), stdOutFd(stdOutFd), stdErrFd(stdErrFd) {}

  friend struct Command;

public:
  Result<ExitStatus> wait() const noexcept;
  Result<CommandOutput> waitWithOutput() const noexcept;
};

struct Command {
  enum class IOConfig : uint8_t {
    Null,
    Inherit,
    Piped,
  };

  std::string command;
  std::vector<std::string> arguments;
  std::filesystem::path workingDirectory;
  IOConfig stdOutConfig = IOConfig::Inherit;
  IOConfig stdErrConfig = IOConfig::Inherit;
  std::vector<std::pair<std::string, std::string>> environment;

  explicit Command(std::string_view cmd) : command(cmd) {}
  Command(std::string_view cmd, std::vector<std::string> args)
      : command(cmd), arguments(std::move(args)) {}

  Command& addArg(const std::string_view arg) {
    arguments.emplace_back(arg);
    return *this;
  }
  Command& addArgs(const std::span<const std::string> args) {
    arguments.insert(arguments.end(), args.begin(), args.end());
    return *this;
  }
  template <typename T>
    requires fmt::is_formattable<T>::value
  Command& addArgs(const std::vector<T>& args) {
    for (const auto& arg : args) {
      arguments.emplace_back(fmt::format("{}", arg));
    }
    return *this;
  }

  Command& setStdOutConfig(IOConfig config) noexcept {
    stdOutConfig = config;
    return *this;
  }
  Command& setStdErrConfig(IOConfig config) noexcept {
    stdErrConfig = config;
    return *this;
  }
  Command& setWorkingDirectory(const std::filesystem::path& dir) {
    workingDirectory = dir;
    return *this;
  }
  Command& setEnv(std::string key, std::string value) {
    environment.emplace_back(std::move(key), std::move(value));
    return *this;
  }

  std::string toString() const;

  Result<Child> spawn() const noexcept;
  Result<CommandOutput> output() const noexcept;
};

} // namespace cabin

template <>
struct fmt::formatter<cabin::ExitStatus> : formatter<std::string> {
  auto format(const cabin::ExitStatus& v, format_context& ctx) const
      -> format_context::iterator;
};

template <>
struct fmt::formatter<cabin::Command> : formatter<std::string> {
  auto format(const cabin::Command& v, format_context& ctx) const
      -> format_context::iterator;
};
