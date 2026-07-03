#include <iostream>

#include "netlib/net.hpp"
#include "netlib/tls.hpp"

int main() {
  std::cout << netlib::fetch("example.org") << "\n";
  std::cout << netlib::secure_fetch("example.org") << "\n";
  return 0;
}
