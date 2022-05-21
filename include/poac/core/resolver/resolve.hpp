#ifndef POAC_CORE_RESOLVER_RESOLVE_HPP_
#define POAC_CORE_RESOLVER_RESOLVE_HPP_

// std
#include <algorithm>
#include <cmath>
#include <iostream>
#include <iterator>
#include <regex>
#include <sstream>
#include <stack>
#include <tuple>
#include <utility>

// external
#include <boost/dynamic_bitset.hpp>
#include <boost/property_tree/json_parser.hpp>
#include <boost/property_tree/ptree.hpp>
#include <boost/range/adaptor/filtered.hpp>
#include <boost/range/adaptor/transformed.hpp>
#include <boost/range/algorithm.hpp>
#include <boost/range/algorithm_ext/push_back.hpp>
#include <boost/range/irange.hpp>
#include <boost/range/join.hpp>
#include <spdlog/spdlog.h> // NOLINT(build/include_order)

// internal
#include <poac/config.hpp>
#include <poac/core/resolver/sat.hpp>
#include <poac/poac.hpp>
#include <poac/util/meta.hpp>
#include <poac/util/net.hpp>
#include <poac/util/semver/semver.hpp>
#include <poac/util/verbosity.hpp>

namespace poac::core::resolver::resolve {

struct WithDeps : std::true_type {};
struct WithoutDeps : std::false_type {};

// Duplicate dependencies should have non-resolved dependencies which contains
// package info having `version` rather than interval generally. We should avoid
// using `std::unordered_map` here so that packages with the same name possibly
// store in DuplicateDeps. Package information does not need dependencies'
// dependencies (meaning that flattened), so the second value of std::pair is
// this type rather than Package and just needs std::string indicating a
// specific version.
template <typename W>
struct DuplicateDeps {};

template <typename W>
using DupDeps = typename DuplicateDeps<W>::type;

template <>
struct DuplicateDeps<WithoutDeps> {
  using type = Vec<std::pair<String, String>>;
};

using Package = std::pair<
    String, // name
    String  // version or interval
    >;

inline Package::first_type&
get_name(Package& package) noexcept {
  return package.first;
}

inline const Package::first_type&
get_name(const Package& package) noexcept {
  return package.first;
}

inline Package::second_type&
get_version(Package& package) noexcept {
  return package.second;
}

inline const Package::second_type&
get_version(const Package& package) noexcept {
  return package.second;
}

inline Package::second_type&
get_interval(Package& package) noexcept {
  return get_version(package);
}

inline const Package::second_type&
get_interval(const Package& package) noexcept {
  return get_version(package);
}

using Deps = Option<DupDeps<WithoutDeps>>;

template <>
struct DuplicateDeps<WithDeps> {
  using type = Vec<std::pair<Package, Deps>>;
};

template <typename W>
using UniqDeps = std::conditional_t<
    W::value, HashMap<Package, Deps, HashPair>,
    // <name, version or interval>
    HashMap<String, String>>;

inline const Package&
get_package(const UniqDeps<WithDeps>::value_type& deps) noexcept {
  return deps.first;
}

String
to_binary_numbers(const i32& x, const usize& digit) {
  return format("{:0{}b}", x, digit);
}

// A ∨ B ∨ C
// A ∨ ¬B ∨ ¬C
// ¬A ∨ B ∨ ¬C
// ¬A ∨ ¬B ∨ C
// ¬A ∨ ¬B ∨ ¬C
Vec<Vec<i32>>
multiple_versions_cnf(const Vec<i32>& clause) {
  return boost::irange(0, 1 << clause.size()) // number of combinations
       | boost::adaptors::transformed([&clause](const i32 i) {
           return boost::dynamic_bitset<>(to_binary_numbers(i, clause.size()));
         }) |
         boost::adaptors::filtered([](const boost::dynamic_bitset<>& bs) {
           return bs.count() != 1;
         }) |
         boost::adaptors::transformed(
             [&clause](const boost::dynamic_bitset<>& bs) -> Vec<i32> {
               return boost::irange(usize{0}, bs.size()) |
                      boost::adaptors::transformed([&clause, &bs](const i32 i) {
                        return bs[i] ? clause[i] * -1 : clause[i];
                      }) |
                      util::meta::containerized;
             }
         ) |
         util::meta::containerized;
}

Vec<Vec<i32>>
create_cnf(const DupDeps<WithDeps>& activated) {
  Vec<Vec<i32>> clauses;
  Vec<i32> already_added;

  auto first = std::cbegin(activated);
  auto last = std::cend(activated);
  for (i32 i = 0; i < static_cast<i32>(activated.size()); ++i) {
    if (util::meta::find(already_added, i)) {
      continue;
    }

    const auto name_lambda = [&](const auto& x) {
      return x.first == activated[i].first;
    };
    // No other packages with the same name as the package currently pointed to
    // exist
    if (const auto count = std::count_if(first, last, name_lambda);
        count == 1) {
      Vec<i32> clause;
      clause.emplace_back(i + 1);
      clauses.emplace_back(clause);

      // index ⇒ deps
      if (!activated[i].second.has_value()) {
        clause[0] *= -1;
        for (const auto& [name, version] : activated[i].second.value()) {
          // It is guaranteed to exist
          clause.emplace_back(
              util::meta::index_of_if(
                  first, last,
                  [&n = name, &v = version](const auto& d) {
                    return d.first.first == n && d.first.second == v;
                  }
              ) +
              1
          );
        }
        clauses.emplace_back(clause);
      }
    } else if (count > 1) {
      Vec<i32> clause;

      for (auto found = first; found != last;
           found = std::find_if(found, last, name_lambda)) {
        const auto index = std::distance(first, found);
        clause.emplace_back(index + 1);
        already_added.emplace_back(index + 1);

        // index ⇒ deps
        if (!found->second.has_value()) {
          Vec<i32> new_clause;
          new_clause.emplace_back(index);
          for (const auto& package : found->second.value()) {
            // It is guaranteed to exist
            new_clause.emplace_back(
                util::meta::index_of_if(
                    first, last,
                    [&package = package](const auto& p) {
                      return p.first.first == package.first &&
                             p.first.second == package.second;
                    }
                ) +
                1
            );
          }
          clauses.emplace_back(new_clause);
        }
        ++found;
      }
      boost::range::push_back(clauses, multiple_versions_cnf(clause));
    }
  }
  return clauses;
}

[[nodiscard]] Result<UniqDeps<WithDeps>, String>
solve_sat(const DupDeps<WithDeps>& activated, const Vec<Vec<i32>>& clauses) {
  // deps.activated.size() == variables
  const Vec<i32> assignments = Try(sat::solve(clauses, activated.size()));
  UniqDeps<WithDeps> resolved_deps{};
  spdlog::debug("SAT");
  for (const auto& a : assignments) {
    spdlog::debug("{} ", a);
    if (a > 0) {
      const auto& [package, deps] = activated[a - 1];
      resolved_deps.emplace(package, deps);
    }
  }
  spdlog::debug(0);
  return Ok(resolved_deps);
}

[[nodiscard]] Result<UniqDeps<WithDeps>, String>
backtrack_loop(const DupDeps<WithDeps>& activated) {
  const auto clauses = create_cnf(activated);
  if (util::verbosity::is_verbose()) {
    for (const auto& c : clauses) {
      for (const auto& l : c) {
        const auto deps = activated[std::abs(l) - 1];
        spdlog::debug(
            "{}-{}: {}, ", get_name(get_package(deps)),
            get_version(get_package(deps)), l
        );
      }
      spdlog::debug("");
    }
  }
  return solve_sat(activated, clauses);
}

template <typename SinglePassRange>
bool
duplicate_loose(const SinglePassRange& rng) {
  const auto first = std::begin(rng);
  const auto last = std::end(rng);
  return std::find_if(first, last, [&](const auto& x) {
           return std::count_if(first, last, [&](const auto& y) {
                    return get_name(get_package(x)) == get_name(get_package(y));
                  }) > 1;
         }) != last;
}

// Interval to multiple versions
// `>=0.1.2 and <3.4.0` -> { 2.4.0, 2.5.0 }
// `latest` -> { 2.5.0 }: (removed)
// name is boost/config, no boost-config
[[nodiscard]] Result<Vec<String>, String>
get_versions_satisfy_interval(const Package& package) {
  // TODO(ken-matsui): (`>1.2 and <=1.3.2` -> NG，`>1.2.0-alpha and <=1.3.2` ->
  // OK) `2.0.0` specific version or `>=0.1.2 and <3.4.0` version interval
  const semver::Interval i(get_interval(package));
  const Vec<String> satisfied_versions =
      Try(util::net::api::versions(get_name(package))) |
      boost::adaptors::filtered([&i](StringRef s) { return i.satisfies(s); }) |
      util::meta::containerized;

  if (satisfied_versions.empty()) {
    return Err(format(
        "`{}: {}` not found; seem dependencies are broken", get_name(package),
        get_interval(package)
    ));
  }
  return Ok(satisfied_versions);
}

using IntervalCache = Vec<std::tuple<
    Package,
    Vec<String> // versions in the interval
    >>;

inline const Package&
get_package(const IntervalCache::value_type& cache) noexcept {
  return std::get<0>(cache);
}

inline const Vec<String>&
get_versions(const IntervalCache::value_type& cache) noexcept {
  return std::get<1>(cache);
}

inline bool
exist_cache_impl(const Package& a, const Package& b) noexcept {
  return get_name(a) == get_name(b) && get_version(a) == get_version(b);
}

template <typename Range>
inline bool
exist_cache(Range&& cache, const Package& package) {
  return util::meta::find_if(
      std::forward<Range>(cache),
      [&package](const auto& c) {
        return exist_cache_impl(package, get_package(c));
      }
  );
}

DupDeps<WithoutDeps>
gather_deps_of_deps(
    const UniqDeps<WithoutDeps>& deps_api_res, IntervalCache& interval_cache
) {
  DupDeps<WithoutDeps> cur_deps_deps;
  for (const auto& package : deps_api_res) {
    // Check if node package is resolved dependency (by interval)
    const auto found_cache =
        boost::range::find_if(interval_cache, [&package](const auto& cache) {
          return exist_cache_impl(package, get_package(cache));
        });

    const auto dep_versions =
        found_cache != interval_cache.cend()
            ? get_versions(*found_cache)
            : get_versions_satisfy_interval(package).unwrap();
    if (found_cache == interval_cache.cend()) {
      // Cache interval and versions pair
      interval_cache.emplace_back(package, dep_versions);
    }
    for (const auto& dep_version : dep_versions) {
      cur_deps_deps.emplace_back(get_name(package), dep_version);
    }
  }
  return cur_deps_deps;
}

void
gather_deps(
    const Package& package, DupDeps<WithDeps>& new_deps,
    IntervalCache& interval_cache
) {
  // Check if root package resolved dependency
  //   (whether the specific version is the same),
  //   and check circulating
  if (exist_cache(new_deps, package)) {
    return;
  }

  // Get dependencies of dependencies
  const UniqDeps<WithoutDeps> deps_api_res =
      util::net::api::deps(get_name(package), get_version(package)).unwrap();
  if (deps_api_res.empty()) {
    new_deps.emplace_back(package, None);
  } else {
    const auto deps_of_deps = gather_deps_of_deps(deps_api_res, interval_cache);

    // Store dependency and the dependency's dependencies.
    new_deps.emplace_back(package, deps_of_deps);

    // Gather dependencies of dependencies of dependencies.
    for (const auto& dep_package : deps_of_deps) {
      gather_deps(dep_package, new_deps, interval_cache);
    }
  }
}

[[nodiscard]] Result<DupDeps<WithDeps>, String>
gather_all_deps(const UniqDeps<WithoutDeps>& deps) {
  DupDeps<WithDeps> duplicate_deps;
  IntervalCache interval_cache;

  // Activate the root of dependencies
  for (const auto& package : deps) {
    // Check whether the packages specified in poac.toml
    //   are already resolved which includes
    //   that package's dependencies and package's versions
    //   by checking whether package's interval is the same.
    if (exist_cache(interval_cache, package)) {
      continue;
    }

    // Get versions using interval
    // FIXME: versions API and deps API are received the almost same responses
    const Vec<String> versions = Try(get_versions_satisfy_interval(package));
    // Cache interval and versions pair
    interval_cache.emplace_back(package, versions);
    for (const String& version : versions) {
      gather_deps(
          std::make_pair(get_name(package), version), duplicate_deps,
          interval_cache
      );
    }
  }
  return Ok(duplicate_deps);
}

} // namespace poac::core::resolver::resolve

#endif // POAC_CORE_RESOLVER_RESOLVE_HPP_
