// Exercises Boost.ConceptCheck: BOOST_CONCEPT_ASSERT on standard and user
// types, BOOST_CONCEPT_REQUIRES on a function template, and a custom concept.
#include <boost/concept/requires.hpp>
#include <boost/concept_check.hpp>

#include <list>
#include <vector>

template <class T>
struct Doublable {
  BOOST_CONCEPT_USAGE(Doublable) { T y = x + x; (void)y; }
 private:
  T x;
};

template <class It>
BOOST_CONCEPT_REQUIRES(((boost::RandomAccessIterator<It>)), (int))
distance_ra(It first, It last) {
  return static_cast<int>(last - first);
}

BOOST_CONCEPT_ASSERT((boost::RandomAccessIterator<int*>));
BOOST_CONCEPT_ASSERT((boost::LessThanComparable<double>));
BOOST_CONCEPT_ASSERT((boost::BidirectionalIterator<std::list<int>::iterator>));
BOOST_CONCEPT_ASSERT((boost::RandomAccessContainer<std::vector<int> >));
BOOST_CONCEPT_ASSERT((Doublable<long>));

int main() {
  std::vector<int> v(5, 1);
  if (distance_ra(v.begin(), v.end()) != 5) return 1;

  int a[3] = {1, 2, 3};
  if (distance_ra(a, a + 3) != 3) return 1;

  boost::function_requires<boost::CopyConstructibleConcept<int> >();
  return 0;
}
