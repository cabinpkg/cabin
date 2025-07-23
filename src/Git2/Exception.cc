#include "Exception.hpp"

#include <git2/deprecated.h>
#include <git2/errors.h>

namespace git2 {

Exception::Exception() {
  if (const git_error* error = giterr_last(); error != nullptr) {
    this->msg += error->message;
    this->cat = static_cast<git_error_t>(error->klass);
    giterr_clear();
  }
}

const char* Exception::what() const noexcept { return this->msg.c_str(); }
git_error_t Exception::category() const noexcept { return this->cat; }

int git2Throw(const int ret) {
  if (ret < GIT_OK) {
    throw Exception();
  }
  return ret;
}

} // namespace git2
