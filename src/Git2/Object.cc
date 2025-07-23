#include "Object.hpp"

#include "Oid.hpp"

#include <git2/object.h>

namespace git2 {

Object::~Object() { git_object_free(this->raw); }

Object::Object(git_object* obj) : raw(obj) {}

Oid Object::id() const { return Oid(git_object_id(this->raw)); }

} // namespace git2
