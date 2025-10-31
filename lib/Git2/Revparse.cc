#include "Git2/Revparse.hpp"

#include <git2/object.h>

namespace git2 {

git_object* Revspec::from() const noexcept { return this->fromObj; }

git_object* Revspec::to() const noexcept { return this->toObj; }

unsigned int Revspec::mode() const noexcept { return this->modeVal; }

} // end namespace git2
