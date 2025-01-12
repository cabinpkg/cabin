#include "Cabin.hpp"
#include "Logger.hpp"

#include <span>

int
main(int argc, char* argv[]) {
  return cabin::cliMain(std::span<char* const>(argv + 1, argv + argc))
      .map_err([](const auto& e) { cabin::logger::error("{}", e); })
      .is_err();
}
