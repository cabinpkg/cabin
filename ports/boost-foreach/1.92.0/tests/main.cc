// Iterates standard containers, arrays, and strings with BOOST_FOREACH and
// BOOST_REVERSE_FOREACH, including modification through a reference.
#include <boost/foreach.hpp>
#include <string>
#include <vector>

int main() {
    std::vector<int> v;
    for (int i = 1; i <= 4; ++i) v.push_back(i);

    int sum = 0;
    BOOST_FOREACH (int x, v) sum += x;
    if (sum != 10) return 1;

    BOOST_FOREACH (int& x, v) x *= 10;
    if (v[0] != 10 || v[3] != 40) return 1;

    std::vector<int> reversed;
    BOOST_REVERSE_FOREACH (int x, v) reversed.push_back(x);
    if (reversed.size() != 4 || reversed[0] != 40 || reversed[3] != 10) return 1;

    const int arr[] = {5, 6, 7};
    int product = 1;
    BOOST_FOREACH (int x, arr) product *= x;
    if (product != 210) return 1;

    int vowels = 0;
    std::string word("boost");
    BOOST_FOREACH (char ch, word) {
        if (ch == 't') break;
        if (ch == 'o') ++vowels;
    }
    if (vowels != 2) return 1;
    return 0;
}
