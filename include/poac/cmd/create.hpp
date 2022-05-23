#ifndef POAC_CMD_CREATE_HPP_
#define POAC_CMD_CREATE_HPP_

// std
#include <algorithm>
#include <fstream>
#include <iostream>
#include <stdexcept>
#include <string>

// external
#include <git2-cpp/git2.hpp>
#include <spdlog/spdlog.h> // NOLINT(build/include_order)
#include <structopt/app.hpp>

// internal
#include <poac/core/validator.hpp>
#include <poac/data/manifest.hpp>
#include <poac/poac.hpp>

namespace poac::cmd::create {

struct Options : structopt::sub_command {
  /// Package name to create a new poac package
  String package_name;

  /// Use a binary (application) template [default]
  Option<bool> bin = false;
  /// Use a library template
  Option<bool> lib = false;
};

using PassingBothBinAndLib =
    Error<"cannot specify both lib and binary outputs">;

enum class ProjectType {
  Bin,
  Lib,
};

String
to_string(ProjectType kind) {
  switch (kind) {
    case ProjectType::Bin:
      return "binary (application)";
    case ProjectType::Lib:
      return "library";
    default:
      unreachable();
  }
}

std::ostream&
operator<<(std::ostream& os, ProjectType kind) {
  return (os << to_string(kind));
}

template <typename T>
ProjectType
opts_to_project_type(T&& opts) {
  opts.bin.value(); // Just check opts has a `.bin` member
  return opts.lib.value() ? ProjectType::Lib : ProjectType::Bin;
}

namespace files {
  inline String
  poac_toml(StringRef project_name) {
    return format(
        "[package]\n"
        "name = \"{}\"\n"
        "version = \"0.1.0\"\n"
        "authors = []\n"
        "edition = 2020\n",
        project_name
    );
  }

  inline const String main_cpp(
      "#include <iostream>\n\n"
      "int main(int argc, char** argv) {\n"
      "  std::cout << \"Hello, world!\" << std::endl;\n"
      "}\n"
  );

  inline String
  include_hpp(StringRef project_name) {
    String project_name_upper_cased{};
    std::transform(
        project_name.cbegin(), project_name.cend(),
        std::back_inserter(project_name_upper_cased), ::toupper
    );

    return format(
        "#ifndef {0}_HPP\n"
        "#define {0}_HPP\n\n"
        "namespace {1} {{\n}}\n\n"
        "#endif // !{0}_HPP\n",
        project_name_upper_cased, project_name
    );
  }
} // namespace files

void
write_to_file(std::ofstream& ofs, const String& fname, StringRef text) {
  ofs.open(fname);
  if (ofs.is_open()) {
    ofs << text;
  }
  ofs.close();
  ofs.clear();
}

Map<fs::path, String>
create_template_files(const ProjectType& type, const String& package_name) {
  switch (type) {
    case ProjectType::Bin:
      fs::create_directories(package_name / "src"_path);
      return {
          {".gitignore", "/target"},
          {data::manifest::name, files::poac_toml(package_name)},
          {"src"_path / "main.cpp", files::main_cpp}};
    case ProjectType::Lib:
      fs::create_directories(package_name / "include"_path / package_name);
      return {
          {".gitignore", "/target\npoac.lock"},
          {data::manifest::name, files::poac_toml(package_name)},
          {"include"_path / package_name / (package_name + ".hpp"),
           files::include_hpp(package_name)},
      };
    default:
      unreachable();
  }
}

[[nodiscard]] Result<void>
create(const Options& opts) {
  std::ofstream ofs;
  const ProjectType type = opts_to_project_type(opts);
  for (auto&& [name, text] : create_template_files(type, opts.package_name)) {
    const String& file_path = (opts.package_name / name).string();
    spdlog::trace("Creating {}", file_path);
    write_to_file(ofs, file_path, text);
  }

  spdlog::trace("Initializing git repository at {}", opts.package_name);
  git2::repository().init(opts.package_name);

  log::status(
      "Created"_bold_green, "{} `{}` package", to_string(type),
      opts.package_name
  );
  return Ok();
}

[[nodiscard]] Result<void>
exec(const Options& opts) {
  if (opts.bin.value() && opts.lib.value()) {
    return Err<PassingBothBinAndLib>();
  }

  namespace validator = core::validator;

  spdlog::trace("Validating the `{}` directory exists", opts.package_name);
  Try(validator::can_create_directory(opts.package_name).map_err(to_anyhow));

  spdlog::trace("Validating the package name `{}`", opts.package_name);
  Try(validator::valid_package_name(opts.package_name).map_err(to_anyhow));

  return create(opts);
}

} // namespace poac::cmd::create

STRUCTOPT(poac::cmd::create::Options, package_name, bin, lib);

#endif // POAC_CMD_CREATE_HPP_
