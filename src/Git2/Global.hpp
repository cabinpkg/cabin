#pragma once

namespace git2 {

struct GlobalState {
  GlobalState();
  ~GlobalState();

  GlobalState(const GlobalState&) = default;
  GlobalState(GlobalState&&) noexcept = default;
  GlobalState& operator=(const GlobalState&) = default;
  GlobalState& operator=(GlobalState&&) noexcept = default;
};

} // namespace git2
