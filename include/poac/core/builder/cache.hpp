#ifndef POAC_CORE_BUILDER_CACHE_HPP
#define POAC_CORE_BUILDER_CACHE_HPP

#include <filesystem>
#include <iostream>
#include <string>
#include <vector>
#include <map>
#include <optional>

#include <poac/core/builder/depends.hpp>
#include <poac/util/misc.hpp>

namespace poac::core::builder::cache {
    std::string to_cache_hash_path(const std::string& s) {
        auto hash_path = io::path::current_build_cache_ts_dir;
        hash_path /= std::filesystem::path(s).relative_path();
        return hash_path.string() + ".hash";
    }

    std::optional<std::map<std::string, std::string>>
    load_timestamps(const std::string& src_cpp_hash) {
        if (!std::filesystem::exists(src_cpp_hash)) {
            return std::nullopt;
        }
        std::ifstream ifs(src_cpp_hash);
        if (!ifs.is_open()) {
            return std::nullopt;
        }

        std::string buff;
        std::map<std::string, std::string> hash;
        while (std::getline(ifs, buff)) {
            const auto list_string = util::misc::split(buff, ": \n");
            hash[list_string[0]] = list_string[1];
        }
        return hash;
    }

    void generate_timestamp(
            const std::string& filename,
            std::map<std::string, std::string>& timestamp)
    {
        const auto last_time = std::filesystem::last_write_time(filename);
        timestamp.emplace(filename, io::path::time_to_string(last_time));
    }

    // *.cpp -> hash
    std::optional<std::map<std::string, std::string>>
    generate_timestamps(
            const options::compile& compile_conf,
            const std::string& source_file,
            const bool verbose)
    {
        if (const auto deps_headers = depends::gen(compile_conf, source_file, verbose))
        {
            std::map<std::string, std::string> hash;
            for (const auto& name : *deps_headers) {
                // Calculate the hash of the source dependent files.
                generate_timestamp(name, hash);
            }
            // Calculate the hash of the source file itself.
            generate_timestamp(source_file, hash);
            return hash;
        }
        return std::nullopt;
    }

    std::vector<std::string>
    check_src_cpp(
            const options::compile& compile_conf,
            std::map<std::string, std::map<std::string, std::string>>& depends_ts,
            const std::vector<std::string>& source_files,
            const bool verbose)
    {
        std::vector<std::string> required_source_files; // FIXME: コンパイルが必要な
        for (const auto& sf : source_files) {
            if (const auto previous_ts = load_timestamps(to_cache_hash_path(sf))) {
                if (const auto current_ts = generate_timestamps(compile_conf, sf, verbose))
                {
                    // Since hash of already existing hash file
                    //  does not match hash of current cpp file,
                    //  it does not exclude it from compilation,
                    //  and generates hash for overwriting.
                    if (*previous_ts != *current_ts) {
                        depends_ts[to_cache_hash_path(sf)] = *current_ts; // TODO: これも，メモリにおかず，どんどんストリームに流す？
                        required_source_files.push_back(sf);
                    }
                }
            }
            else {
                // Since hash file does not exist, generates hash and compiles source file.
                if (const auto cur_hash = generate_timestamps(compile_conf, sf, verbose))
                {
                    depends_ts[to_cache_hash_path(sf)] = *cur_hash;
                    required_source_files.push_back(sf);
                }
            }
        }
        return required_source_files;
    }




    void is_required_compile() {}


    // TODO: storeと，load, compare(check) => cacheのAPIをわかりやすくしたい．StoreとLoadのみで，わかりやすく
} // end namespace
#endif // POAC_CORE_BUILDER_CACHE_HPP
