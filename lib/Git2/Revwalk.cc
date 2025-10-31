#include "Git2/Revwalk.hpp"

#include "Git2/Exception.hpp"
#include "Git2/Oid.hpp"
#include "Git2/Repository.hpp"

#include <git2/revwalk.h>
#include <string>

namespace git2 {

Revwalk::Revwalk(const Repository& repo) {
  git2Throw(git_revwalk_new(&this->raw, repo.raw));
}
Revwalk::~Revwalk() noexcept { git_revwalk_free(this->raw); }

Revwalk& Revwalk::reset() {
  git_revwalk_reset(this->raw);
  return *this;
}

Revwalk& Revwalk::setSorting(unsigned int sortMode) {
  git_revwalk_sorting(this->raw, sortMode);
  return *this;
}

Revwalk& Revwalk::simplifyFirstParent() {
  git_revwalk_simplify_first_parent(this->raw);
  return *this;
}

Revwalk& Revwalk::push(const Oid& oid) {
  git2Throw(git_revwalk_push(this->raw, oid.raw));
  return *this;
}

Revwalk& Revwalk::pushHead() {
  git2Throw(git_revwalk_push_head(this->raw));
  return *this;
}

Revwalk& Revwalk::pushGlob(const std::string& glob) {
  git2Throw(git_revwalk_push_glob(this->raw, glob.c_str()));
  return *this;
}

Revwalk& Revwalk::pushRange(const std::string& range) {
  git2Throw(git_revwalk_push_range(this->raw, range.c_str()));
  return *this;
}

Revwalk& Revwalk::pushRef(const std::string& reference) {
  git2Throw(git_revwalk_push_ref(this->raw, reference.c_str()));
  return *this;
}

Revwalk& Revwalk::hide(const Oid& oid) {
  git2Throw(git_revwalk_hide(this->raw, oid.raw));
  return *this;
}

Revwalk& Revwalk::hideHead() {
  git2Throw(git_revwalk_hide_head(this->raw));
  return *this;
}

Revwalk& Revwalk::hideGlob(const std::string& glob) {
  git2Throw(git_revwalk_hide_glob(this->raw, glob.c_str()));
  return *this;
}

Revwalk& Revwalk::hideRef(const std::string& reference) {
  git2Throw(git_revwalk_hide_ref(this->raw, reference.c_str()));
  return *this;
}

} // end namespace git2
