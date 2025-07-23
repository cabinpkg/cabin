#include "Repository.hpp"

#include "Config.hpp"
#include "Exception.hpp"
#include "Oid.hpp"

#include <git2/checkout.h>
#include <git2/clone.h>
#include <git2/ignore.h>
#include <git2/object.h>
#include <git2/oid.h>
#include <git2/refs.h>
#include <git2/repository.h>
#include <git2/revparse.h>
#include <string>

namespace git2 {

Repository::~Repository() { git_repository_free(this->raw); }

Repository& Repository::open(const std::string& path) {
  git2Throw(git_repository_open(&this->raw, path.c_str()));
  return *this;
}
Repository& Repository::openBare(const std::string& path) {
  git2Throw(git_repository_open_bare(&this->raw, path.c_str()));
  return *this;
}

Repository& Repository::init(const std::string& path) {
  git2Throw(git_repository_init(&this->raw, path.c_str(), false));
  return *this;
}
Repository& Repository::initBare(const std::string& path) {
  git2Throw(git_repository_init(&this->raw, path.c_str(), true));
  return *this;
}

bool Repository::isIgnored(const std::string& path) const {
  int ignored = 0;
  git2Throw(git_ignore_path_is_ignored(&ignored, this->raw, path.c_str()));
  return static_cast<bool>(ignored);
}

Repository& Repository::clone(const std::string& url, const std::string& path,
                              const git_clone_options* opts) {
  git2Throw(git_clone(&this->raw, url.c_str(), path.c_str(), opts));
  return *this;
}

Object Repository::revparseSingle(const std::string& spec) const {
  git_object* obj = nullptr;
  git2Throw(git_revparse_single(&obj, this->raw, spec.c_str()));
  return git2::Object(obj);
}

Repository& Repository::setHeadDetached(const Oid& oid) {
  git2Throw(git_repository_set_head_detached(this->raw, oid.raw));
  return *this;
}

Repository& Repository::checkoutHead(bool force) {
  git_checkout_options opts;
  git2Throw(git_checkout_options_init(&opts, GIT_CHECKOUT_OPTIONS_VERSION));
  opts.checkout_strategy = force ? GIT_CHECKOUT_FORCE : GIT_CHECKOUT_SAFE;
  git2Throw(git_checkout_head(this->raw, &opts));
  return *this;
}

Oid Repository::refNameToId(const std::string& refname) const {
  git_oid oid;
  git2Throw(git_reference_name_to_id(&oid, this->raw, refname.c_str()));
  return Oid(oid);
}

git2::Config Repository::config() const {
  git_config* cfg = nullptr;
  git2Throw(git_repository_config(&cfg, this->raw));
  return git2::Config(cfg);
}

}  // end namespace git2
