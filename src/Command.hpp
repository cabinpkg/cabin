#pragma once

#include "Rustify/Result.hpp"

#include <cstdint>
#include <filesystem>
#include <ostream>
#include <span>
#include <string>
#include <string_view>
#include <sys/types.h>
#include <utility>
#include <vector>

namespace cabin {

struct CommandOutput {
  const int exitCode;
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
  Result<int> wait() const noexcept;
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
