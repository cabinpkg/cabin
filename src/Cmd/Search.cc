#include "Search.hpp"

#include "Cli.hpp"
#include "Diag.hpp"
#include "Rustify/Result.hpp"

#include <cstddef>
#include <cstdlib>
#include <curl/curl.h>
#include <iomanip>
#include <iostream>
#include <nlohmann/json.hpp>
#include <string>
#include <string_view>

namespace cabin {

static Result<void> searchMain(CliArgsView args);

const Subcmd SEARCH_CMD =
    Subcmd{ "search" }
        .setDesc("Search for packages in the registry")
        .addOpt(
            Opt{ "--per-page" }
                .setDesc("Number of results to show per page")
                .setPlaceholder("<NUM>")
                .setDefault("10")
        )
        .addOpt(
            Opt{ "--page" }
                .setDesc("Page number of results to show")
                .setPlaceholder("<NUM>")
                .setDefault("1")
        )
        .setArg(Arg{ "name" })
        .setMainFn(searchMain);

struct SearchArgs {
  std::string name;
  std::size_t perPage = 10;
  std::size_t page = 1;
};

static std::size_t
writeCallback(
    void* contents, std::size_t size, std::size_t nmemb, std::string* userp
) {
  userp->append(static_cast<char*>(contents), size * nmemb);
  return size * nmemb;
}

static nlohmann::json
searchPackages(const SearchArgs& args) {
  nlohmann::json req;
  req["query"] =
#include "GraphQL/SearchPackages.gql"
      ;
  req["variables"]["name"] = "%" + args.name + "%";
  req["variables"]["limit"] = args.perPage;
  req["variables"]["offset"] = (args.page - 1) * args.perPage;

  const std::string reqStr = req.dump();
  std::string resStr;

  CURL* curl = curl_easy_init();
  if (!curl) {
    Diag::error("curl_easy_init() failed");
    return EXIT_FAILURE;
  }

  curl_easy_setopt(curl, CURLOPT_URL, "https://cabin.hasura.app/v1/graphql");
  curl_easy_setopt(curl, CURLOPT_WRITEFUNCTION, writeCallback);
  curl_easy_setopt(curl, CURLOPT_WRITEDATA, &resStr);
  curl_easy_setopt(curl, CURLOPT_POST, 1L);
  curl_easy_setopt(curl, CURLOPT_POSTFIELDS, reqStr.c_str());
  curl_easy_perform(curl);  // TODO: Handle CURLCode

  curl_easy_cleanup(curl);

  const nlohmann::json res = nlohmann::json::parse(resStr);
  const nlohmann::json packages = res["data"]["packages"];
  return packages;
}

static void
printTable(const nlohmann::json& packages) {
  constexpr int tableWidth = 80;
  constexpr int nameWidth = 30;
  constexpr int verWidth = 10;

  std::cout << std::left << std::setw(nameWidth) << "Name"
            << std::setw(verWidth) << "Version" << "Description" << '\n';
  std::cout << std::string(tableWidth, '-') << '\n';
  for (const auto& package : packages) {
    const std::string name = package["name"];
    const std::string version = package["version"];
    const std::string description = package["description"];

    std::cout << std::left << std::setw(nameWidth) << name
              << std::setw(verWidth) << version << description << '\n';
  }
}

static Result<void>
searchMain(const CliArgsView args) {
  SearchArgs searchArgs;
  for (auto itr = args.begin(); itr != args.end(); ++itr) {
    const std::string_view arg = *itr;

    const auto control = Try(Cli::handleGlobalOpts(itr, args.end(), "search"));
    if (control == Cli::Return) {
      return Ok();
    } else if (control == Cli::Continue) {
      continue;
    } else if (arg == "--per-page") {
      Ensure(itr + 1 < args.end(), "missing argument for `--per-page`");
      searchArgs.perPage = std::stoul(std::string(*++itr));
    } else if (arg == "--page") {
      Ensure(itr + 1 < args.end(), "missing argument for `--page`");
      searchArgs.page = std::stoul(std::string(*++itr));
    } else if (searchArgs.name.empty()) {
      searchArgs.name = *itr;
    } else {
      return SEARCH_CMD.noSuchArg(arg);
    }
  }
  Ensure(!searchArgs.name.empty(), "missing package name");

  const nlohmann::json packages = searchPackages(searchArgs);
  if (packages.empty()) {
    Diag::warn("no packages found");
    return Ok();
  }

  printTable(packages);
  return Ok();
}

}  // namespace cabin
