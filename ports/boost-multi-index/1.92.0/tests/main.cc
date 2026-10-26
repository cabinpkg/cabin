// Exercises boost::multi_index_container with a unique ordered index on id and a
// non-unique ordered index on name: insert, duplicate rejection, and lookup by both keys.
#include <boost/multi_index_container.hpp>
#include <boost/multi_index/member.hpp>
#include <boost/multi_index/ordered_index.hpp>
#include <string>

struct employee {
    int id;
    std::string name;
    employee(int i, const std::string& n) : id(i), name(n) {}
};

struct by_name {};

typedef boost::multi_index_container<
    employee,
    boost::multi_index::indexed_by<
        boost::multi_index::ordered_unique<boost::multi_index::member<employee, int, &employee::id> >,
        boost::multi_index::ordered_non_unique<boost::multi_index::tag<by_name>,
                                               boost::multi_index::member<employee, std::string, &employee::name> > > >
    employee_set;

int main() {
    employee_set es;
    es.insert(employee(3, "carol"));
    es.insert(employee(1, "alice"));
    es.insert(employee(2, "alice"));
    if (es.insert(employee(1, "dup")).second) return 1;
    if (es.size() != 3) return 1;

    if (es.begin()->id != 1) return 1;
    employee_set::iterator it = es.find(3);
    if (it == es.end() || it->name != "carol") return 1;

    employee_set::index<by_name>::type& names = es.get<by_name>();
    if (names.count("alice") != 2) return 1;
    if (names.begin()->name != "alice") return 1;
    names.erase("alice");
    if (es.size() != 1 || es.begin()->id != 3) return 1;
    return 0;
}
