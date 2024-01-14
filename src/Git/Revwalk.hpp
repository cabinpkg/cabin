#pragma once

#include "../Rustify.hpp"
#include "Oid.hpp"
#include "Repository.hpp"

#include <git2/revwalk.h>

namespace git2 {

struct Revwalk {
  git_revwalk* raw = nullptr;

  Revwalk() = delete;
  explicit Revwalk(const Repository&);
  ~Revwalk() noexcept;

  Revwalk(const Revwalk&) = delete;
  Revwalk& operator=(const Revwalk&) = delete;
  Revwalk(Revwalk&&) = default;
  Revwalk& operator=(Revwalk&&) = default;

  /// Reset a revwalk to allow re-configuring it.
  ///
  /// The revwalk is automatically reset when iteration of its commits
  /// completes.
  Revwalk& reset();

  /// Set the order in which commits are visited.
  Revwalk& setSorting(unsigned int sort_mode);

  /// Simplify the history by first-parent
  ///
  /// No parents other than the first for each commit will be enqueued.
  Revwalk& simplifyFirstParent();

  /// Mark a commit to start traversal from.
  ///
  /// The given OID must belong to a committish on the walked repository.
  ///
  /// The given commit will be used as one of the roots when starting the
  /// revision walk. At least one commit must be pushed onto the walker before
  /// a walk can be started.
  Revwalk& push(const Oid&);

  /// Push the repository's HEAD
  ///
  /// For more information, see `push`.
  Revwalk& pushHead();

  /// Push matching references
  ///
  /// The OIDs pointed to by the references that match the given glob pattern
  /// will be pushed to the revision walker.
  ///
  /// A leading 'refs/' is implied if not present as well as a trailing `/ \ *`
  /// if the glob lacks '?', ' \ *' or '['.
  ///
  /// Any references matching this glob which do not point to a committish
  /// will be ignored.
  Revwalk& pushGlob(const StringRef glob);

  /// Push and hide the respective endpoints of the given range.
  ///
  /// The range should be of the form `<commit>..<commit>` where each
  /// `<commit>` is in the form accepted by `revparse_single`. The left-hand
  /// commit will be hidden and the right-hand commit pushed.
  Revwalk& pushRange(const StringRef range);

  /// Push the OID pointed to by a reference
  ///
  /// The reference must point to a committish.
  Revwalk& pushRef(const StringRef reference);

  /// Mark a commit as not of interest to this revwalk.
  Revwalk& hide(const Oid& oid);

  /// Hide the repository's HEAD
  ///
  /// For more information, see `hide`.
  Revwalk& hideHead();

  /// Hide matching references.
  ///
  /// The OIDs pointed to by the references that match the given glob pattern
  /// and their ancestors will be hidden from the output on the revision walk.
  ///
  /// A leading 'refs/' is implied if not present as well as a trailing `/ \ *`
  /// if the glob lacks '?', ' \ *' or '['.
  ///
  /// Any references matching this glob which do not point to a committish
  /// will be ignored.
  Revwalk& hideGlob(const StringRef glob);

  /// Hide the OID pointed to by a reference.
  ///
  /// The reference must point to a committish.
  Revwalk& hideRef(const StringRef reference);
};

} // end namespace git2
