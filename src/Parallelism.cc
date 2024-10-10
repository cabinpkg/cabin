#include "Parallelism.hpp"

#include "Logger.hpp"

#include <cstddef>
#include <memory>
#include <tbb/global_control.h>
#include <thread>

size_t
numThreads() noexcept {
  const unsigned int numThreads = std::thread::hardware_concurrency();
  if (numThreads > 1) {
    return numThreads;
  }
  return 1;
}

struct ParallelismState {
  // ParallelismState is a singleton
  ParallelismState(const ParallelismState&) = delete;
  ParallelismState& operator=(const ParallelismState&) = delete;
  ParallelismState(ParallelismState&&) noexcept = delete;
  ParallelismState& operator=(ParallelismState&&) noexcept = delete;
  ~ParallelismState() noexcept = default;

  void set(size_t numThreads) noexcept {
    if (numThreads == 0) {
      logger::warn("requested parallelism of 0, capping at 1");
      numThreads = 1;
    }

    status = std::make_unique<tbb::global_control>(
        tbb::global_control::max_allowed_parallelism, numThreads
    );
  }
  size_t get() const noexcept {
    // NOLINTNEXTLINE(readability-static-accessed-through-instance)
    return status->active_value(tbb::global_control::max_allowed_parallelism);
  }

  static ParallelismState& instance() noexcept {
    static ParallelismState instance;
    return instance;
  }

private:
  std::unique_ptr<tbb::global_control> status;

  ParallelismState() noexcept
      : status(std::make_unique<tbb::global_control>(
            tbb::global_control::max_allowed_parallelism, numThreads()
        )) {}
};

void
setParallelism(const size_t numThreads) noexcept {
  ParallelismState::instance().set(numThreads);
}

size_t
getParallelism() noexcept {
  return ParallelismState::instance().get();
}

bool
isParallel() noexcept {
  return getParallelism() > 1;
}
