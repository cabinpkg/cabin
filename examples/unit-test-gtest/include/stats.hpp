#pragma once

#include <vector>

// Throws std::invalid_argument on an empty series.
double mean(const std::vector<double> &values);

// Takes its input by value: the median needs a sorted copy anyway.
// Throws std::invalid_argument on an empty series.
double median(std::vector<double> values);
