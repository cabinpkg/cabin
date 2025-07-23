#include "Global.hpp"

#include "Exception.hpp"

#include <git2/global.h>

namespace git2 {

GlobalState::GlobalState() { git2Throw(git_libgit2_init()); }
GlobalState::~GlobalState() { git2Throw(git_libgit2_shutdown()); }

}  // namespace git2
