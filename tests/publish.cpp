#define BOOST_TEST_MAIN
#include <boost/test/included/unit_test.hpp>
#include <boost/filesystem.hpp>

#include <string>
#include <string_view>
#include <fstream>

#include <poac/core/except.hpp>
#include <poac/io/config.hpp>
#include <poac/opts/publish.hpp>
#include <poac/config.hpp>

// std::optional<core::except::Error> verify_cpp_version(const std::uint_fast16_t& cpp_version)
BOOST_AUTO_TEST_CASE( poac_opts_publish_verify_cpp_version_test )
{
    using poac::opts::publish::verify_cpp_version;

    BOOST_CHECK( !verify_cpp_version(98).has_value() );
    BOOST_CHECK( !verify_cpp_version(3).has_value() );
    BOOST_CHECK( !verify_cpp_version(11).has_value() );
    BOOST_CHECK( !verify_cpp_version(14).has_value() );
    BOOST_CHECK( !verify_cpp_version(17).has_value() );
    BOOST_CHECK( !verify_cpp_version(20).has_value() );

    BOOST_CHECK( verify_cpp_version(1).has_value() );
    BOOST_CHECK( verify_cpp_version(6).has_value() );
    BOOST_CHECK( verify_cpp_version(9).has_value() );
    BOOST_CHECK( verify_cpp_version(12).has_value() );
    BOOST_CHECK( verify_cpp_version(15).has_value() );
    BOOST_CHECK( verify_cpp_version(18).has_value() );
    BOOST_CHECK( verify_cpp_version(21).has_value() );
    BOOST_CHECK( verify_cpp_version(99).has_value() );
}

// Failure on CI
// std::string get_local_commit_sha(const std::string& version)
//BOOST_AUTO_TEST_CASE( poac_opts_publish_get_local_commit_sha_test )
//{
//    using poac::opts::publish::get_local_commit_sha;
//    BOOST_CHECK_THROW(
//            get_local_commit_sha("unknown"),
//            poac::core::except::error
//    );
//    BOOST_CHECK( get_local_commit_sha("0.2.1") == "353368f90544bb160b258a9cc1ecba8d467c4020" );
//}

// PackageType get_package_type(const std::optional<io::config::Config>& config)
BOOST_AUTO_TEST_CASE( poac_opts_publish_get_package_type_test )
{
    namespace fs = boost::filesystem;
    using poac::opts::publish::get_package_type;
    using poac::io::lockfile::PackageType;
    using poac::io::config::load;

//    const fs::path config_path = fs::current_path() / "poac.yml";
//    {
//        std::ofstream ofs(config_path.string());
//        ofs << "cpp_version: 17";
//    }
//    BOOST_CHECK( get_package_type(load()) == PackageType::HeaderOnlyLib );
//    fs::remove(config_path);
//
//    {
//        std::ofstream ofs(config_path.string());
//        ofs << "build:\n"
//               "  bin: true";
//    }
//    BOOST_CHECK( get_package_type(load()) == PackageType::Application );
//    fs::remove(config_path);
//
//    {
//        std::ofstream ofs(config_path.string());
//        ofs << "build:\n"
//               "  lib: true";
//    }
//    BOOST_CHECK( get_package_type(load()) == PackageType::BuildReqLib );
//    fs::remove(config_path);
//
//    {
//        std::ofstream ofs(config_path.string());
//        ofs << "build:\n"
//               "  hoge: fuga";
//    }
//    BOOST_CHECK( get_package_type(load()) == PackageType::BuildReqLib );
//    fs::remove(config_path);
//
//    {
//        std::ofstream ofs(config_path.string());
//        ofs << "build:\n"
//               "  bin: false";
//    }
//    BOOST_CHECK( get_package_type(load()) == PackageType::BuildReqLib );
//    fs::remove(config_path);
//
//    {
//        std::ofstream ofs(config_path.string());
//        ofs << "build:\n"
//               "  lib: false";
//    }
//    BOOST_CHECK( get_package_type(load()) == PackageType::BuildReqLib );
//    fs::remove(config_path);
//
//    {
//        std::ofstream ofs(config_path.string());
//        ofs << "build:\n"
//               "  bin: false\n"
//               "  lib: false";
//    }
//    BOOST_CHECK( get_package_type(load()) == PackageType::BuildReqLib );
//    fs::remove(config_path);
//
//    {
//        std::ofstream ofs(config_path.string());
//        ofs << "build:\n"
//               "  bin: true\n"
//               "  lib: true";
//    }
//    BOOST_CHECK( get_package_type(load()) == PackageType::Application );
//    fs::remove(config_path);
//    {
//        std::ofstream ofs(config_path.string());
//        ofs << "build:\n"
//               "  lib: true\n"
//               "  bin: true";
//    }
//    BOOST_CHECK( get_package_type(load()) == PackageType::Application );
//    fs::remove(config_path);
}

// std::optional<std::string> get_license(const std::string& full_name, const std::string& version)
BOOST_AUTO_TEST_CASE( poac_opts_publish_get_license_test )
{
    using poac::opts::publish::get_license;
    BOOST_CHECK( get_license("poacpm/poac", "0.2.1").value() == "GNU General Public License v3.0" );
    BOOST_CHECK( get_license("poacpm/api.poac.pm", "master").value() == "MIT License" );
}

// std::uint_fast16_t get_cpp_version()
BOOST_AUTO_TEST_CASE( poac_opts_publish_get_cpp_version_test )
{
    namespace fs = boost::filesystem;
    using poac::opts::publish::get_cpp_version;

//    const fs::path config_path = fs::current_path() / "poac.yml";
//    {
//        std::ofstream ofs(config_path.string());
//        ofs << "cpp_version: 17";
//    }
//
//    BOOST_CHECK( get_cpp_version(poac::io::config::load()) == 17 );
//    fs::remove(config_path);
}

// std::optional<std::string> get_description(const std::string& full_name)
BOOST_AUTO_TEST_CASE( poac_opts_publish_get_description_test )
{
    using poac::opts::publish::get_description;
    BOOST_CHECK( get_description("poacpm/poac").value() == "Package manager for C++" );
    BOOST_CHECK( !get_description("matken11235/to_TFRecord").has_value() );
}
