// Exercises boost::heap::fibonacci_heap ordering and handle-based update, and
// boost::heap::priority_queue draining in descending order.
#include <boost/heap/fibonacci_heap.hpp>
#include <boost/heap/priority_queue.hpp>

int main() {
    boost::heap::fibonacci_heap<int> fib;
    auto handle = fib.push(3);
    fib.push(7);
    fib.push(1);
    if (fib.top() != 7) return 1;
    fib.update(handle, 10);
    if (fib.top() != 10) return 1;
    fib.pop();
    if (fib.top() != 7 || fib.size() != 2) return 1;

    boost::heap::priority_queue<int> pq;
    for (int v : {5, 2, 9, 4}) pq.push(v);
    const int expected[] = {9, 5, 4, 2};
    for (int e : expected) {
        if (pq.empty() || pq.top() != e) return 1;
        pq.pop();
    }
    return pq.empty() ? 0 : 1;
}
