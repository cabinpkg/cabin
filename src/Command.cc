#include "Command.hpp"

#include "Exception.hpp"
#include "Rustify.hpp"

#include <array>
#include <cstddef>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/wait.h>
#include <unistd.h>
#include <vector>

int
Child::wait() const {
  int status;
  if (waitpid(pid, &status, 0) == -1) {
    close(stdoutfd);
    throw PoacError("waitpid() failed");
  }

  close(stdoutfd);

  const int exitCode = WEXITSTATUS(status);
  return exitCode;
}

CommandOutput
Child::wait_with_output() const {
  constexpr std::size_t bufferSize = 128;
  std::array<char, bufferSize> buffer{};
  std::string output;

  FILE* stream = fdopen(stdoutfd, "r");
  if (stream == nullptr) {
    close(stdoutfd);
    throw PoacError("fdopen() failed");
  }

  while (fgets(buffer.data(), buffer.size(), stream) != nullptr) {
    output += buffer.data();
  }

  fclose(stream);

  int status;
  if (waitpid(pid, &status, 0) == -1) {
    throw PoacError("waitpid() failed");
  }

  const int exitCode = WEXITSTATUS(status);
  return { output, exitCode };
}

Child
Command::spawn() const {
  int stdoutPipe[2];

  if (stdoutConfig == StdioConfig::Piped) {
    if (pipe(stdoutPipe) == -1) {
      throw PoacError("pipe() failed");
    }
  }

  pid_t pid = fork();
  if (pid == -1) {
    throw PoacError("fork() failed");
  } else if (pid == 0) {
    if (stdoutConfig == StdioConfig::Piped) {
      close(stdoutPipe[0]); // child doesn't read

      // redirect stdout to pipe
      dup2(stdoutPipe[1], 1);
      close(stdoutPipe[1]);
    } else if (stdoutConfig == StdioConfig::Null) {
      int nullfd = open("/dev/null", O_WRONLY);
      dup2(nullfd, 1);
      close(nullfd);
    }

    std::vector<char*> args;
    args.push_back(const_cast<std::string&>(command).data());
    for (std::string& arg : const_cast<std::vector<std::string>&>(arguments)) {
      args.push_back(arg.data());
    }
    args.push_back(nullptr);

    if (!working_directory.empty()) {
      if (chdir(working_directory.c_str()) == -1) {
        throw PoacError("chdir() failed");
      }
    }

    if (execvp(command.data(), args.data()) == -1) {
      throw PoacError("execvp() failed");
    }
    unreachable();
  } else {
    if (stdoutConfig == StdioConfig::Piped) {
      close(stdoutPipe[1]); // parent doesn't write

      return Child(pid, stdoutPipe[0]);
    } else {
      return Child(pid, /* stdin */ 0);
    }
  }
}

CommandOutput
Command::output() const {
  Command cmd = *this;
  cmd.setStdoutConfig(StdioConfig::Piped);
  return cmd.spawn().wait_with_output();
}

std::string
Command::to_string() const {
  std::string res = command;
  for (const std::string& arg : arguments) {
    res += ' ' + arg;
  }
  return res;
}

std::ostream&
operator<<(std::ostream& os, const Command& cmd) {
  return os << cmd.to_string();
}
