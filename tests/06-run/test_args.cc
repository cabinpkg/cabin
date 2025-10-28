#include <iostream>
int main(int argc, char* argv[]) {
  std::cout << "argc=" << argc << '\n';
  for (int i = 1; i < argc; ++i) {
    std::cout << "arg[" << i << "]=" << argv[i] << '\n';
  }
}