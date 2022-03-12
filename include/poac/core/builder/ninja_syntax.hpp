// C++ class to generate .ninja files.
// This file is based on ninja_syntax.py from:
// https://github.com/ninja-build/ninja/blob/master/misc/ninja_syntax.py

#ifndef POAC_CORE_BUILDER_NINJA_SYNTAX_HPP
#define POAC_CORE_BUILDER_NINJA_SYNTAX_HPP

// std
#include <cassert>
#include <cstddef>
#include <cstdint>
#include <filesystem>
#include <optional>
#include <ostream>
#include <string>
#include <string_view>
#include <unordered_map>
#include <vector>

// external
#include <boost/algorithm/string.hpp>
#include <boost/range/algorithm_ext/push_back.hpp>
#include <boost/regex.hpp>
#include <fmt/core.h>

// internal
#include <poac/util/meta.hpp>
#include <poac/util/pretty.hpp>

namespace poac::core::builder::ninja_syntax {
    inline std::filesystem::path
    escape_path(std::filesystem::path p) {
        std::string s = p.string();
        boost::replace_all(s, "$ ", "$$ ");
        boost::replace_all(s, " ", "$ ");
        boost::replace_all(s, ":", "$:");
        return s;
    }

    /// Escape a string such that it can be embedded into a Ninja file without
    /// further interpretation.
    inline void
    escape(std::string& s) {
        assert(s.find('\n') == std::string::npos); // Ninja syntax does not allow newlines
        // We only have one special metacharacter: '$'.
        boost::replace_all(s, "$", "$$");
    }

    /// Expand a string containing $vars as Ninja would.
    ///
    /// Note: doesn't handle the full Ninja variable syntax, but it's enough
    /// to make configure.py's use of it work.
    using variables_t = std::unordered_map<std::string, std::string>;
    std::string
    expand(const std::string& text, const variables_t& vars, const variables_t& local_vars={}) {
        const auto exp = [&](const boost::smatch& m) {
            using namespace std::literals::string_literals;

            const std::string var = m[1].str();
            if (var == "$") {
                return "$"s;
            }
            return local_vars.contains(var)
                ? local_vars.at(var)
                : vars.contains(var)
                       ? vars.at(var)
                       : ""s;
        };
        return boost::regex_replace(text, boost::regex("\\$(\\$|\\w*)"), exp);
    }

    /// ref: https://stackoverflow.com/a/46379136
    std::string operator*(const std::string& s, std::size_t n) {
        std::string result;
        result.reserve(s.size() * n);
        for (std::size_t i = 0; i < n; ++i) {
            result += s;
        }
        return result;
    }

    struct rule_set_t {
        std::optional<std::string> description = std::nullopt;
        std::optional<std::string> depfile = std::nullopt;
        bool generator = false;
        std::optional<std::string> pool = std::nullopt;
        bool restat = false;
        std::optional<std::string> rspfile = std::nullopt;
        std::optional<std::string> rspfile_content = std::nullopt;
        std::optional<std::string> deps = std::nullopt;
    };

    struct build_set_t {
        std::optional<std::filesystem::path> inputs = std::nullopt;
        std::optional<std::filesystem::path> implicit = std::nullopt;
        std::optional<std::filesystem::path> order_only = std::nullopt;
        std::optional<std::unordered_map<std::string, std::string>> variables = std::nullopt;
        std::optional<std::filesystem::path> implicit_outputs = std::nullopt;
        std::optional<std::string> pool = std::nullopt;
        std::optional<std::string> dyndep = std::nullopt;
    };

    template <typename Ostream>
    requires util::meta::derived_from<Ostream, std::ostream>
    class writer {
        Ostream output;
        std::size_t width;

        /// Returns the number of '$' characters right in front of s[i].
        std::size_t
        count_dollars_before_index(std::string_view s, std::size_t i) const {
            std::size_t dollar_count = 0;
            std::size_t dollar_index = i - 1;
            while (dollar_index > 0 && s[dollar_index] == '$') {
                dollar_count += 1;
                dollar_index -= 1;
            }
            return dollar_count;
        }

        // Export this function for testing
#if __has_include(<boost/ut.hpp>)
    public:
#endif
        /// Write 'text' word-wrapped at self.width characters.
        void _line(std::string text, std::size_t indent = 0) {
            std::string leading_space = std::string("  ") * indent;

            while (leading_space.length() + text.length() > width) {
                // The text is too wide; wrap if possible.

                // Find the rightmost space that would obey our width constraint and
                // that's not an escaped space.
                std::int32_t available_space = width - leading_space.length() - 2; // " $".length() == 2
                std::int32_t space = available_space;
                do {
                    space = text.rfind(' ', space);
                } while (!(space < 0 || count_dollars_before_index(text, space) % 2 == 0));

                if (space < 0) {
                    // No such space; just use the first unescaped space we can find.
                    space = available_space - 1;
                    do {
                        space = text.find(' ', space + 1);
                    } while (!(space < 0 || count_dollars_before_index(text, space) % 2 == 0));
                }
                if (space < 0) {
                    // Give up on breaking.
                    break;
                }

                output << leading_space + text.substr(0, space) + " $\n";
                text = text.substr(space + 1);

                // Subsequent lines are continuations, so indent them.
                leading_space = std::string("  ") * (indent + 2);
            }
            output << leading_space + text + '\n';
        }

    public:
        explicit writer(Ostream&& o, std::size_t w = 78) : output(std::move(o)), width(w) {}

        inline std::string
        get_value() const {
            return output.str();
        }

        inline void
        newline() {
            output << '\n';
        }

        inline void
        comment(const std::string& text) {
            for (const auto& line : util::pretty::textwrap(text, width - 2)) {
                output << "# " + line + '\n';
            }
        }

        inline void
        variable(std::string_view key, std::string_view value, std::size_t indent = 0) {
            if (value.empty()) {
                return;
            }
            _line(fmt::format("{} = {}", key, value), indent);
        }

        inline void
        variable(std::string_view key, std::vector<std::string> values, std::size_t indent = 0) {
            const std::string value = boost::algorithm::join_if(values, " ", [](const auto& s){
                return !s.empty();
            });
            _line(fmt::format("{} = {}", key, value), indent);
        }

        inline void
        pool(std::string_view name, std::string_view depth) {
            _line(fmt::format("pool {}", name));
            variable("depth", depth, 1);
        }

        void
        rule(std::string_view name, std::string_view command, const rule_set_t& rule_set) {
            _line(fmt::format("rule {}", name));
            variable("command", command, 1);
            if (rule_set.description.has_value()) {
                variable("description", rule_set.description.value(), 1);
            }
            if (rule_set.depfile.has_value()) {
                variable("depfile", rule_set.depfile.value(), 1);
            }
            if (rule_set.generator) {
                variable("generator", "1", 1);
            }
            if (rule_set.pool.has_value()) {
                variable("pool", rule_set.pool.value(), 1);
            }
            if (rule_set.restat) {
                variable("restat", "1", 1);
            }
            if (rule_set.rspfile.has_value()) {
                variable("rspfile", rule_set.rspfile.value(), 1);
            }
            if (rule_set.rspfile_content.has_value()) {
                variable("rspfile_content", rule_set.rspfile_content.value(), 1);
            }
            if (rule_set.deps.has_value()) {
                variable("deps", rule_set.deps.value(), 1);
            }
        }

        std::vector<std::filesystem::path>
        build(
            const std::vector<std::filesystem::path>& outputs,
            std::string_view rule,
            const build_set_t& build_set
        ) {
            std::vector<std::string> out_outputs;
            for (const auto& o : outputs) {
                out_outputs.emplace_back(escape_path(o).string());
            }

            std::vector<std::string> all_inputs;
            if (build_set.inputs.has_value()) {
                for (const auto& i : build_set.inputs.value()) {
                    all_inputs.emplace_back(escape_path(i).string());
                }
            }

            if (build_set.implicit.has_value()) {
                std::vector<std::string> implicit;
                for (const auto& i : build_set.implicit.value()) {
                    implicit.emplace_back(escape_path(i).string());
                }
                all_inputs.emplace_back("|");
                boost::push_back(all_inputs, implicit);
            }
            if (build_set.order_only.has_value()) {
                std::vector<std::string> order_only;
                for (const auto& o : build_set.order_only.value()) {
                    order_only.emplace_back(escape_path(o).string());
                }
                all_inputs.emplace_back("||");
                boost::push_back(all_inputs, order_only);
            }
            if (build_set.implicit_outputs.has_value()) {
                std::vector<std::string> implicit_outputs;
                for (const auto& i : build_set.implicit_outputs.value()) {
                    implicit_outputs.emplace_back(escape_path(i).string());
                }
                out_outputs.emplace_back("|");
                boost::push_back(out_outputs, implicit_outputs);
            }

            _line(fmt::format(
                "build {}: {} {}",
                boost::algorithm::join(out_outputs, " "),
                rule,
                boost::algorithm::join(all_inputs, " ")
            ));

            if (build_set.pool.has_value()) {
                _line(fmt::format("  pool = {}", build_set.pool.value()));
            }
            if (build_set.dyndep.has_value()) {
                _line(fmt::format("  dyndep = {}", build_set.dyndep.value()));
            }

            if (build_set.variables.has_value()) {
                for (const auto& [key, val] : build_set.variables.value()) {
                    variable(key, val, 1);
                }
            }

            return outputs;
        }

        inline void
        include(const std::filesystem::path& path) {
            _line(fmt::format("include {}", path.string()));
        }

        inline void
        subninja(const std::filesystem::path& path) {
            _line(fmt::format("subninja {}", path.string()));
        }

        inline void
        default_(const std::vector<std::filesystem::path>& paths) {
            _line(fmt::format("default {}", boost::algorithm::join(paths, " ").string()));
        }

        inline void
        close() {
            output.close();
        }

        friend std::ostream&
        operator<<(std::ostream &os, const writer<Ostream>& w) {
            return os << w.get_value();
        }
    };
} // end namespace

#endif // POAC_CORE_BUILDER_NINJA_SYNTAX_HPP
