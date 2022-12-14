// std
#include <array>
#include <map>
#include <string>
#include <tuple>
#include <vector>

// external
#include <boost/ut.hpp>

// internal
#include <poac/util/meta.hpp>

auto main() -> int {
  using namespace std::literals::string_literals;
  using namespace boost::ut;

  // 1. std::optional<std::size_t> index_of(const SinglePassRange& rng, const T&
  // t)
  // 2. inline auto index_of(InputIterator first, InputIterator last, const T&
  // value)
  "test index_of"_test = [] {
    using poac::util::meta::index_of;

    std::vector<std::string> test_case{"0", "1", "2"};
    expect(index_of(test_case.begin(), test_case.end(), "1") == 1_i);
    expect(
        index_of(test_case.begin(), test_case.end(), "10") == 3_i
    ); // out of range
    expect(index_of(test_case.cbegin(), test_case.cend(), "0") == 0_i);
  };

  // inline auto index_of_if(InputIterator first, InputIterator last, Predicate
  // pred)
  "test index_of_if"_test = [] {
    using poac::util::meta::index_of_if;

    const std::vector<std::string> test_case{"0", "1", "2"};
    expect(
        index_of_if(
            test_case.cbegin(), test_case.cend(),
            [](auto& x) { return x == "0"; }
        )
        == 0_i
    );
  };

  // bool duplicate(const SinglePassRange& rng)
  "test duplicate"_test = [] {
    using poac::util::meta::duplicate;

    std::vector<std::string> test_case{"0", "0", "2"};
    expect(duplicate(test_case));

    test_case = {"0", "1", "2"};
    expect(!duplicate(test_case));
  };

  // 1. std::vector<T> ptree_to_vector(const U& pt, const K& key)
  // 2. std::vector<T> ptree_to_vector(const U &pt)
  "test to_vec"_test = [] {
    using poac::util::meta::to_vec;

    boost::property_tree::ptree pt;
    const std::vector<std::string> test_case{"0", "1", "2"};

    boost::property_tree::ptree children;
    {
      boost::property_tree::ptree child;
      child.put("", "0");
      children.push_back(std::make_pair("", child));
    }
    {
      boost::property_tree::ptree child;
      child.put("", "1");
      children.push_back(std::make_pair("", child));
    }
    {
      boost::property_tree::ptree child;
      child.put("", "2");
      children.push_back(std::make_pair("", child));
    }
    pt.add_child("data", children);

    expect(eq(to_vec<std::string>(pt, "data"), test_case)); // 1
    expect(eq(to_vec<std::string>(children), test_case)); // 2
  };

  "test are_all_same"_test = [] {
    using poac::util::meta::AreAllSame;
    using poac::util::meta::ARE_ALL_SAME_V;

    expect(constant<AreAllSame<int, int, int>::value>);
    expect(constant<ARE_ALL_SAME_V<int, int, int>>);
    expect(constant<std::negation_v<AreAllSame<int, std::string, int>>>);
    expect(constant<std::negation_v<AreAllSame<std::string, int, int>>>);
    expect(constant<std::negation_v<AreAllSame<int, int, std::string>>>);
  };

  "test is_specialization"_test = [] {
    using poac::util::meta::IsSpecialization;

    expect(constant<IsSpecialization<std::vector<int>, std::vector>::value>);
    expect(constant<IsSpecialization<std::map<int, int>, std::map>::value>);
    expect(constant<
           IsSpecialization<std::map<int, std::vector<int>>, std::map>::value>);
    expect(constant<std::negation_v<IsSpecialization<
               std::map<int, std::vector<int>>, std::vector>>>);
  };

  "test is_tuple"_test = [] {
    using poac::util::meta::IsTuple;
    using poac::util::meta::IS_TUPLE_V;

    expect(constant<IS_TUPLE_V<std::tuple<int>>>);
    expect(constant<IS_TUPLE_V<std::tuple<int, std::string>>>);
    expect(constant<std::negation_v<IsTuple<std::vector<int>>>>);
  };

  "test to_array"_test = [] {
    using poac::util::meta::to_array;

    constexpr std::array<int, 3> TEST_CASE{0, 1, 2};
    constexpr std::tuple<int, int, int> RES1 = std::make_tuple(0, 1, 2);
    constexpr std::array<int, 3> RES = to_array(RES1);
    expect(constant<RES == TEST_CASE>);
  };
}
