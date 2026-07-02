#include "stats.hpp"

#include <algorithm>
#include <cstddef>
#include <numeric>
#include <stdexcept>

double mean(const std::vector<double> &values) {
    if (values.empty()) {
        throw std::invalid_argument("mean of an empty series");
    }
    return std::accumulate(values.begin(), values.end(), 0.0)
           / static_cast<double>(values.size());
}

double median(std::vector<double> values) {
    if (values.empty()) {
        throw std::invalid_argument("median of an empty series");
    }
    std::sort(values.begin(), values.end());
    const std::size_t mid = values.size() / 2;
    if (values.size() % 2 == 1) {
        return values[mid];
    }
    return (values[mid - 1] + values[mid]) / 2.0;
}
