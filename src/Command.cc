#include "Command.hpp"

#include "Rustify/Result.hpp"

#include <algorithm>
#include <array>
#include <cstdio>
#include <cstdlib>
#include <fcntl.h>
#include <fmt/format.h>
#include <string>
#include <sys/select.h>
#include <sys/wait.h>
#include <unistd.h>
#include <vector>

namespace cabin {

constexpr std::size_t BUFFER_SIZE = 128;

bool
ExitStatus::exitedNormally() const noexcept {
  return WIFEXITED(rawStatus);
}
bool
ExitStatus::killedBySignal() const noexcept {
  return WIFSIGNALED(rawStatus);
}
bool
ExitStatus::stoppedBySignal() const noexcept {
  return WIFSTOPPED(rawStatus);
}
int
ExitStatus::exitCode() const noexcept {
  return WEXITSTATUS(rawStatus);
}
int
ExitStatus::termSignal() const noexcept {
  return WTERMSIG(rawStatus);
}
int
ExitStatus::stopSignal() const noexcept {
  return WSTOPSIG(rawStatus);
}
bool
ExitStatus::coreDumped() const noexcept {
  return WCOREDUMP(rawStatus);
}

// Successful only if normally exited with code 0
bool
ExitStatus::success() const noexcept {
  return exitedNormally() && exitCode() == 0;
}

std::string
ExitStatus::toString() const {
  if (exitedNormally()) {
    return fmt::format("exited with code {}", exitCode());
  } else if (killedBySignal()) {
    return fmt::format("killed by signal {}{}", termSignal(),
                       coreDumped() ? " (core dumped)" : "");
  } else if (stoppedBySignal()) {
    return fmt::format("stopped by signal {}", stopSignal());
  }
  return "unknown status";
}

Result<ExitStatus>
Child::wait() const noexcept {
  int status{};
  if (waitpid(pid, &status, 0) == -1) {
    if (stdOutFd != -1) {
      close(stdOutFd);
    }
    if (stdErrFd != -1) {
      close(stdErrFd);
    }
    Bail("waitpid() failed");
  }

  if (stdOutFd != -1) {
    close(stdOutFd);
  }
  if (stdErrFd != -1) {
    close(stdErrFd);
  }

  return Ok(ExitStatus{ status });
}

Result<CommandOutput>
Child::waitWithOutput() const noexcept {
  std::string stdOutOutput;
  std::string stdErrOutput;

  int maxfd = -1;
  fd_set readfds;

  // Determine the maximum file descriptor
  maxfd = std::max(maxfd, stdOutFd);
  maxfd = std::max(maxfd, stdErrFd);

  bool stdOutEOF = (stdOutFd == -1);
  bool stdErrEOF = (stdErrFd == -1);

  while (!stdOutEOF || !stdErrEOF) {
    FD_ZERO(&readfds);
    if (!stdOutEOF) {
      FD_SET(stdOutFd, &readfds);
    }
    if (!stdErrEOF) {
      FD_SET(stdErrFd, &readfds);
    }

    const int ret = select(maxfd + 1, &readfds, nullptr, nullptr, nullptr);
    if (ret == -1) {
      if (stdOutFd != -1) {
        close(stdOutFd);
      }
      if (stdErrFd != -1) {
        close(stdErrFd);
      }
      Bail("select() failed");
    }

    // Read from stdout if available
    if (!stdOutEOF && FD_ISSET(stdOutFd, &readfds)) {
      std::array<char, BUFFER_SIZE> buffer{};
      const ssize_t count = read(stdOutFd, buffer.data(), buffer.size());
      if (count == -1) {
        if (stdOutFd != -1) {
          close(stdOutFd);
        }
        if (stdErrFd != -1) {
          close(stdErrFd);
        }
        Bail("read() failed on stdout");
      } else if (count == 0) {
        stdOutEOF = true;
        close(stdOutFd);
      } else {
        stdOutOutput.append(buffer.data(), static_cast<std::size_t>(count));
      }
    }

    // Read from stderr if available
    if (!stdErrEOF && FD_ISSET(stdErrFd, &readfds)) {
      std::array<char, BUFFER_SIZE> buffer{};
      const ssize_t count = read(stdErrFd, buffer.data(), buffer.size());
      if (count == -1) {
        if (stdOutFd != -1) {
          close(stdOutFd);
        }
        if (stdErrFd != -1) {
          close(stdErrFd);
        }
        Bail("read() failed on stderr");
      } else if (count == 0) {
        stdErrEOF = true;
        close(stdErrFd);
      } else {
        stdErrOutput.append(buffer.data(), static_cast<std::size_t>(count));
      }
    }
  }

  int status{};
  if (waitpid(pid, &status, 0) == -1) {
    Bail("waitpid() failed");
  }
  return Ok(CommandOutput{ .exitStatus = ExitStatus{ status },
                           .stdOut = stdOutOutput,
                           .stdErr = stdErrOutput });
}

Result<Child>
Command::spawn() const noexcept {
  std::array<int, 2> stdOutPipe{};
  std::array<int, 2> stdErrPipe{};

  // Set up stdout pipe if needed
  if (stdOutConfig == IOConfig::Piped) {
    if (pipe(stdOutPipe.data()) == -1) {
      Bail("pipe() failed for stdout");
    }
  }
  // Set up stderr pipe if needed
  if (stdErrConfig == IOConfig::Piped) {
    if (pipe(stdErrPipe.data()) == -1) {
      Bail("pipe() failed for stderr");
    }
  }

  const pid_t pid = fork();
  if (pid == -1) {
    Bail("fork() failed");
  } else if (pid == 0) {
    // Child process

    // Redirect stdout
    if (stdOutConfig == IOConfig::Piped) {
      close(stdOutPipe[0]);  // Child doesn't read from stdout pipe
      dup2(stdOutPipe[1], STDOUT_FILENO);
      close(stdOutPipe[1]);
    } else if (stdOutConfig == IOConfig::Null) {
      // NOLINTNEXTLINE(cppcoreguidelines-pro-type-vararg)
      const int nullfd = open("/dev/null", O_WRONLY);
      dup2(nullfd, STDOUT_FILENO);
      close(nullfd);
    }

    // Redirect stderr
    if (stdErrConfig == IOConfig::Piped) {
      close(stdErrPipe[0]);  // Child doesn't read from stderr pipe
      dup2(stdErrPipe[1], STDERR_FILENO);
      close(stdErrPipe[1]);
    } else if (stdErrConfig == IOConfig::Null) {
      // NOLINTNEXTLINE(cppcoreguidelines-pro-type-vararg)
      const int nullfd = open("/dev/null", O_WRONLY);
      dup2(nullfd, STDERR_FILENO);
      close(nullfd);
    }

    // Prepare arguments
    std::vector<std::vector<char>> argBuffers;
    std::vector<char*> args;

    // Add command
    argBuffers.emplace_back(command.begin(), command.end());
    argBuffers.back().push_back('\0');
    args.push_back(argBuffers.back().data());

    // Add arguments
    for (const std::string& arg : arguments) {
      argBuffers.emplace_back(arg.begin(), arg.end());
      argBuffers.back().push_back('\0');
      args.push_back(argBuffers.back().data());
    }
    args.push_back(nullptr);

    if (!workingDirectory.empty()) {
      if (chdir(workingDirectory.c_str()) == -1) {
        perror("chdir() failed");
        _exit(1);
      }
    }

    // Execute the command
    if (execvp(command.c_str(), args.data()) == -1) {
      perror("execvp() failed");
      _exit(1);
    }

    __builtin_unreachable();
  } else {
    // Parent process

    // Close unused pipe ends
    if (stdOutConfig == IOConfig::Piped) {
      close(stdOutPipe[1]);  // Parent doesn't write to stdout pipe
    }
    if (stdErrConfig == IOConfig::Piped) {
      close(stdErrPipe[1]);  // Parent doesn't write to stderr pipe
    }

    // Return the Child object with appropriate file descriptors
    return Ok(Child{ pid, stdOutConfig == IOConfig::Piped ? stdOutPipe[0] : -1,
                     stdErrConfig == IOConfig::Piped ? stdErrPipe[0] : -1 });
  }
}

Result<CommandOutput>
Command::output() const noexcept {
  Command cmd = *this;
  cmd.setStdOutConfig(IOConfig::Piped);
  cmd.setStdErrConfig(IOConfig::Piped);
  return Try(cmd.spawn()).waitWithOutput();
}

std::string
Command::toString() const {
  std::string res = command;
  for (const std::string& arg : arguments) {
    res += ' ' + arg;
  }
  return res;
}

}  // namespace cabin

auto
fmt::formatter<cabin::ExitStatus>::format(const cabin::ExitStatus& v,
                                          format_context& ctx) const
    -> format_context::iterator {
  return formatter<std::string>::format(v.toString(), ctx);
}

auto
fmt::formatter<cabin::Command>::format(const cabin::Command& v,
                                       format_context& ctx) const
    -> format_context::iterator {
  return formatter<std::string>::format(v.toString(), ctx);
}
