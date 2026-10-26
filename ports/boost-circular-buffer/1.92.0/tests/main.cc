// Exercises Boost.CircularBuffer: fixed-capacity push_back/push_front that
// overwrites the oldest element, linearize, and resizing the capacity.
#include <boost/circular_buffer.hpp>

#include <numeric>

int main() {
  boost::circular_buffer<int> cb(3);
  if (!cb.empty() || cb.capacity() != 3) return 1;

  cb.push_back(1);
  cb.push_back(2);
  cb.push_back(3);
  if (!cb.full() || cb.front() != 1 || cb.back() != 3) return 1;

  cb.push_back(4);  // overwrites the oldest element (1)
  if (cb.size() != 3 || cb[0] != 2 || cb[2] != 4) return 1;
  if (std::accumulate(cb.begin(), cb.end(), 0) != 9) return 1;

  cb.push_front(9);  // overwrites the newest element (4)
  if (cb[0] != 9 || cb.back() != 3) return 1;

  int* p = cb.linearize();
  if (p[0] != 9 || p[1] != 2 || p[2] != 3) return 1;

  cb.pop_front();
  cb.set_capacity(5);
  cb.push_back(7);
  if (cb.size() != 3 || cb.capacity() != 5 || cb.back() != 7) return 1;
  return 0;
}
