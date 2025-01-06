#pragma once

#include <cstddef>
#include <string>

std::size_t numThreads() noexcept;
inline const std::string NUM_DEFAULT_THREADS = std::to_string(numThreads());

void setParallelism(std::size_t numThreads) noexcept;
std::size_t getParallelism() noexcept;
bool isParallel() noexcept;
