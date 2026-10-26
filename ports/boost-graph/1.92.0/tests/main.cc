// Exercises Boost.Graph's compiled readers: a weighted digraph parsed
// from DOT (read_graphviz_new.cpp) feeds Dijkstra's shortest paths, and
// a GraphML document (graphml.cpp) loads vertex and edge properties.
#include <boost/graph/adjacency_list.hpp>
#include <boost/graph/dijkstra_shortest_paths.hpp>
#include <boost/graph/graphml.hpp>
#include <boost/graph/graphviz.hpp>
#include <sstream>
#include <string>
#include <vector>

struct Vertex {
    std::string name;
};
struct Edge {
    int weight = 0;
};
typedef boost::adjacency_list<boost::vecS, boost::vecS, boost::directedS, Vertex, Edge> Graph;

int main() {
    Graph g;
    boost::dynamic_properties dp(boost::ignore_other_properties);
    dp.property("node_id", boost::get(&Vertex::name, g));
    dp.property("weight", boost::get(&Edge::weight, g));
    std::istringstream dot("digraph { a -> b [weight=4]; a -> c [weight=1]; c -> b [weight=2]; b -> d [weight=5]; }");
    if (!boost::read_graphviz(dot, g, dp, "node_id")) return 1;
    if (boost::num_vertices(g) != 4 || boost::num_edges(g) != 4 || g[0].name != "a") return 2;

    // An iterator_property_map, not a raw pointer: MSVC's permissive
    // lookup resolves `get` on an `int*` map to boost::detail::get.
    std::vector<int> dist(boost::num_vertices(g));
    boost::dijkstra_shortest_paths(
        g, boost::vertex(0, g),
        boost::weight_map(boost::get(&Edge::weight, g))
            .distance_map(boost::make_iterator_property_map(dist.begin(), boost::get(boost::vertex_index, g))));
    for (std::size_t v = 0; v < dist.size(); ++v) {
        const std::string& n = g[v].name;
        const int want = n == "a" ? 0 : n == "b" ? 3 : n == "c" ? 1 : 8;
        if (dist[v] != want) return 3;
    }

    const char* xml =
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>"
        "<graphml xmlns=\"http://graphml.graphdrawing.org/xmlns\">"
        "<key id=\"k0\" for=\"node\" attr.name=\"name\" attr.type=\"string\"/>"
        "<key id=\"k1\" for=\"edge\" attr.name=\"weight\" attr.type=\"int\"/>"
        "<graph id=\"G\" edgedefault=\"directed\">"
        "<node id=\"n0\"><data key=\"k0\">x</data></node>"
        "<node id=\"n1\"><data key=\"k0\">y</data></node>"
        "<edge source=\"n0\" target=\"n1\"><data key=\"k1\">7</data></edge>"
        "<edge source=\"n1\" target=\"n0\"><data key=\"k1\">-3</data></edge>"
        "</graph></graphml>";
    Graph h;
    boost::dynamic_properties hp(boost::ignore_other_properties);
    hp.property("name", boost::get(&Vertex::name, h));
    hp.property("weight", boost::get(&Edge::weight, h));
    std::istringstream in(xml);
    boost::read_graphml(in, h, hp);
    if (boost::num_vertices(h) != 2 || boost::num_edges(h) != 2) return 4;
    if (h[0].name != "x" || h[1].name != "y") return 5;
    const std::pair<Graph::edge_descriptor, bool> e = boost::edge(1, 0, h);
    return e.second && h[e.first].weight == -3 ? 0 : 6;
}
