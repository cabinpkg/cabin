#include "Cabin.hpp"

int
main(int argc, char* argv[]) {
  return cabin::cabinMain(argc, argv).is_err();
}
