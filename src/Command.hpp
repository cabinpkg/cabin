#pragma once

#include "Rustify/Result.hpp"

#include <cstdint>
#include <cstdlib>
#include <filesystem>
#include <fmt/core.h>
#include <fmt/ostream.h>
#include <ostream>
#include <span>
#include <string>
#include <string_view>
#include <sys/types.h>
#include <sys/wait.h>
#include <utility>
#include <vector>

namespace cabin {

class ExitStatus {
  int rawStatus;  // Original status from waitpid

public:
  ExitStatus() noexcept : rawStatus(EXIT_SUCCESS) {}
  explicit ExitStatus(int status) noexcept : rawStatus(status) {}

  bool exitedNormally() const noexcept {
    return WIFEXITED(rawStatus);
  }
  bool killedBySignal() const noexcept {
    return WIFSIGNALED(rawStatus);
  }
  bool stoppedBySignal() const noexcept {
    return WIFSTOPPED(rawStatus);
  }
  int exitCode() const noexcept {
    return WEXITSTATUS(rawStatus);
  }
  int termSignal() const noexcept {
    return WTERMSIG(rawStatus);
  }
  int stopSignal() const noexcept {
    return WSTOPSIG(rawStatus);
  }
  bool coreDumped() const noexcept {
    return WCOREDUMP(rawStatus);
  }

  // Successful only if normally exited with code 0
  bool success() const noexcept {
    return exitedNormally() && exitCode() == 0;
  }

  std::string toString() const {
    if (exitedNormally()) {
      return fmt::format("exited with code {}", exitCode());
    } else if (killedBySignal()) {
      return fmt::format(
          "killed by signal {}{}", termSignal(),
          coreDumped() ? " (core dumped)" : ""
      );
    } else if (stoppedBySignal()) {
      return fmt::format("stopped by signal {}", stopSignal());
    }
    return "unknown status";
  }

  friend std::ostream& operator<<(std::ostream& os, const ExitStatus& status) {
    os << status.toString();
    return os;
  }
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

  std::string toString() const;

  Result<Child> spawn() const noexcept;
  Result<CommandOutput> output() const noexcept;
};

std::ostream& operator<<(std::ostream& os, const Command& cmd);

}  // namespace cabin

template <>
struct fmt::formatter<cabin::ExitStatus> : ostream_formatter {};
