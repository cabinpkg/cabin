#ifndef POAC_CORE_BUILDER_CHAIN_CMAKE_HPP
#define POAC_CORE_BUILDER_CHAIN_CMAKE_HPP

#include <iostream>
#include <string>

#include <poac/core/except.hpp>
#include <poac/io/path.hpp>
#include <poac/util/shell.hpp>

namespace poac::core::builder::chain {
    struct cmake {
        io::filesystem::path base_path;

        bool build() { // TODO: builderと同じinterfaceであるべき
            util::shell cmd("cd " + base_path.string());
            if (!io::filesystem::exists(base_path / "_build")) {
                cmd &= "mkdir _build";
            }
            cmd &= "cd _build";
            util::shell cmake_cmd("cmake ..");
//            for (const auto& [key, val] : cmake_args) { // TODO: -Dhoge 渡したい
//                cmake_cmd.env(key, val);
//            }
            cmd &= cmake_cmd;
            cmd &= util::shell("make");

            return cmd.exec_ignore();
        }

        explicit cmake(const io::filesystem::path& base_path = io::filesystem::current) {
            if (!io::filesystem::exists(base_path / "CMakeLists.txt")) {
                throw except::error(
                        except::msg::does_not_exist((base_path / "CMakeLists.txt").string()));
            }
            this->base_path = base_path;
        }
    };
} // end namespace
#endif // POAC_CORE_BUILDER_CHAIN_CMAKE_HPP
