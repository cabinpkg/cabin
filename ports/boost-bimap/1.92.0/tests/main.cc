// Exercises Boost.Bimap: inserting relations and looking them up from both the
// left and right views, plus duplicate rejection on a set-of-relations bimap.
#include <boost/bimap.hpp>

#include <string>

int main() {
  typedef boost::bimap<std::string, int> dictionary;
  typedef dictionary::value_type relation;

  dictionary d;
  d.insert(relation("one", 1));
  d.insert(relation("two", 2));
  d.insert(relation("three", 3));
  if (d.size() != 3) return 1;

  if (d.left.at("two") != 2) return 1;
  if (d.right.at(3) != "three") return 1;

  dictionary::left_const_iterator l = d.left.find("one");
  if (l == d.left.end() || l->second != 1) return 1;
  dictionary::right_const_iterator r = d.right.find(2);
  if (r == d.right.end() || r->second != "two") return 1;

  // Both sides are unique sets, so a clash on either side is rejected.
  if (d.insert(relation("uno", 1)).second) return 1;
  if (d.insert(relation("one", 9)).second) return 1;

  d.right.erase(1);
  if (d.left.count("one") != 0 || d.size() != 2) return 1;
  if (d.right.begin()->first != 2) return 1;
  return 0;
}
