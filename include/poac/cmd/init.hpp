#ifndef POAC_CMD_INIT_HPP
#define POAC_CMD_INIT_HPP

// std
#include <iostream>
#include <fstream>
#include <filesystem>
#include <optional>
#include <string>

// external
#include <fmt/core.h>
#include <mitama/result/result.hpp>
#include <mitama/anyhow/anyhow.hpp>
#include <mitama/thiserror/thiserror.hpp>
#include <spdlog/spdlog.h>
#include <structopt/app.hpp>

// internal
#include <poac/cmd/new.hpp>
#include <poac/core/validator.hpp>
#include <poac/util/termcolor2/termcolor2.hpp>
#include <poac/util/termcolor2/literals_extra.hpp>

namespace poac::cmd::init {
    namespace anyhow = mitama::anyhow;
    namespace thiserror = mitama::thiserror;

    struct Options: structopt::sub_command {
        /// Use a binary (application) template [default]
        std::optional<bool> bin = false;
        /// Use a library template
        std::optional<bool> lib = false;
    };

    class InitError {
        template <thiserror::fixed_string S, class ...T>
        using error = thiserror::error<S, T...>;

    public:
        using PassingBothBinAndLib =
            error<"cannot specify both lib and binary outputs">;
        using AlreadyInitialized =
            error<"cannot initialize an existing poac package">;
    };

    [[nodiscard]] anyhow::result<void>
    init(const Options& opts, std::string_view package_name) {
        spdlog::trace("Creating ./poac.toml");
        std::ofstream ofs_config("poac.toml");

        const bool is_bin = !opts.lib.value();
        if (is_bin) {
            ofs_config << _new::files::poac_toml(package_name);
        } else {
            ofs_config << _new::files::poac_toml(package_name);
        }

        using termcolor2::color_literals::operator""_bold_green;
        spdlog::info(
            "{:>25} {} `{}` package",
            "Created"_bold_green,
            is_bin ? "binary (application)" : "library",
            package_name
        );
        return mitama::success();
    }

    [[nodiscard]] anyhow::result<void>
    exec(const Options& opts) {
        if (opts.bin.value() && opts.lib.value()) {
            return anyhow::failure<InitError::PassingBothBinAndLib>();
        } else if (core::validator::required_config_exists().is_ok()) {
            return anyhow::failure<InitError::AlreadyInitialized>();
        }

        const std::string package_name = std::filesystem::current_path().stem().string();
        spdlog::trace("Validating the package name `{}`", package_name);
        MITAMA_TRY(
            core::validator::valid_package_name(package_name)
            .map_err([](const std::string& e){ return anyhow::anyhow(e); })
        );

        return init(opts, package_name);
    }
} // end namespace

#endif // !POAC_CMD_INIT_HPP
