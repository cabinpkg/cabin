// Exercises Boost.Intrusive list and set containers linking caller-owned nodes
// through base hooks, without allocating.
#include <boost/intrusive/list.hpp>
#include <boost/intrusive/set.hpp>

struct item : public boost::intrusive::list_base_hook<>, public boost::intrusive::set_base_hook<> {
    explicit item(int v) : value(v) {}
    int value;
    friend bool operator<(const item& a, const item& b) { return a.value < b.value; }
};

int main() {
    item nodes[] = {item(5), item(1), item(3)};
    boost::intrusive::list<item> lst;
    boost::intrusive::set<item> st;
    for (int i = 0; i < 3; ++i) {
        lst.push_back(nodes[i]);
        st.insert(nodes[i]);
    }
    if (lst.size() != 3 || &lst.front() != &nodes[0]) return 1;
    if (st.begin()->value != 1 || st.rbegin()->value != 5) return 1;
    if (st.find(item(3)) == st.end() || &*st.find(item(3)) != &nodes[2]) return 1;
    lst.erase(lst.iterator_to(nodes[1]));
    if (lst.size() != 2 || lst.back().value != 3) return 1;
    lst.clear();
    st.clear();
    return 0;
}
