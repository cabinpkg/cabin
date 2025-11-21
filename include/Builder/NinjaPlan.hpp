#pragma once

#include <filesystem>
#include <string>
#include <utility>
#include <vector>

namespace cabin {

struct NinjaEdge {
  std::vector<std::string> outputs;
  std::string rule;
  std::vector<std::string> inputs;
  std::vector<std::string> implicitInputs;
  std::vector<std::string> orderOnlyInputs;
  std::vector<std::pair<std::string, std::string>> bindings;
};

struct NinjaToolchain {
  std::string cxx;
  std::string cxxFlags;
  std::string defines;
  std::string includes;
  std::string ldFlags;
  std::string libs;
  std::string archiver;
};

class NinjaPlan {
public:
  explicit NinjaPlan(std::filesystem::path outBasePath);

  void reset();
  void addEdge(NinjaEdge edge);
  void addDefaultTarget(std::string target);
  void setTestTargets(std::vector<std::string> testTargets);
  void writeFiles(const NinjaToolchain& toolchain) const;

private:
  void writeBuildNinja() const;
  void writeConfigNinja(const NinjaToolchain& toolchain) const;
  void writeRulesNinja() const;
  void writeTargetsNinja() const;

  std::filesystem::path outBasePath_;
  std::vector<NinjaEdge> edges_;
  std::vector<std::string> defaultTargets_;
  std::vector<std::string> testTargets_;
};

} // namespace cabin
