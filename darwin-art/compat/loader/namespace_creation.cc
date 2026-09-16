#include "namespace_handles.h"
#include "namespace_operation.h"

namespace darwin_art::loader {
android_namespace_t* NamespaceHandles::ParentForCaller(android_namespace_t* parent,
    const LinkerImageLease* caller, std::string* error) {
  if (error) error->clear();
  NamespaceOperation operation(configured_->registry());
  if (!operation) {
    if (error) *error = "cannot enter linker operation";
    return nullptr;
  }
  std::lock_guard lock(mutex_);
  uint64_t id = 0;
  if (parent) id = Resolve(parent);
  else if (caller) {
    if (darwin_art_linker_image_registered_primary_namespace(configured_->registry(), caller, &id) != 0)
      id = 0;
  } else id = anonymous_id_ ? anonymous_id_ : configured_->Find("default");
  if (!id) {
    if (error) *error = "parent or caller is not owned by this linker";
    return nullptr;
  }
  return Intern(id);
}

android_namespace_t* NamespaceHandles::Anonymous() {
  std::lock_guard lock(mutex_);
  return Intern(anonymous_id_ ? anonymous_id_ : configured_->Find("default"));
}

android_namespace_t* NamespaceHandles::CreateChild(android_namespace_t* parent,
    bool isolated, bool shared, bool anonymous, const char* search,
    const char* defaults, const char* permitted, std::string* error, const char* name) {
  if (error) error->clear();
  auto fail = [error](const char* detail) -> android_namespace_t* {
    if (error) *error = detail;
    return nullptr;
  };
  NamespaceOperation operation(configured_->registry());
  if (!operation) return fail("cannot enter linker operation");
  std::lock_guard lock(mutex_);
  const auto parent_id = Resolve(parent);
  if (!parent_id) return fail("effective parent namespace is not owned");
  if (anonymous && anonymous_id_) return fail("anonymous namespace already assigned");
  uint64_t id = 0;
  // AOSP create_namespace clones paths, images and links only for SHARED.
  // Otherwise inherit the parent's shared image group, not its paths/links.
  const auto status = shared
      ? darwin_art_linker_namespace_create(configured_->registry(), isolated,
          search, defaults, permitted, parent_id, &id)
      : darwin_art_linker_namespace_inherit_group(configured_->registry(), isolated,
          search, defaults, permitted, parent_id,
          parent_id == configured_->Find("default"), &id);
  if (status != 0) return fail("namespace creation failed");
  auto* handle = Intern(id);
  if (name) tokens_.at(id)->name = name;
  if (anonymous) anonymous_id_ = id;
  return handle;
}

std::string NamespaceHandles::Name(android_namespace_t* handle) {
  std::lock_guard lock(mutex_);
  const auto id = Resolve(handle);
  return id ? tokens_.at(id)->name : std::string{};
}
}  // namespace darwin_art::loader
