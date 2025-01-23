#include "Cabin.hpp"

int
main(int argc, char* argv[]) {
  return cabin::cliMain(argc, argv).is_err();
}
