#include "Driver.hpp"

int
main(int argc, char* argv[]) {
  return cabin::run(argc, argv).is_err();
}
