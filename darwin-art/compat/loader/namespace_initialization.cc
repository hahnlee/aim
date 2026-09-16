#include "namespace_load.h"
#include "namespace_operation.h"
#include "namespace_open_reference.h"

namespace darwin_art::loader {
bool NamespaceHandles::PublishAndInitialize(const LoadedGraph& graph, bool global,
    std::string* error, android_namespace_t* open_namespace,
    const DarwinArtElfFileIdentity* open_identity, LinkerImageLease** open_output, bool nodelete) {
  if (open_output) *open_output = nullptr;
  if ((nodelete && !open_output) || (open_output != nullptr) != (open_namespace != nullptr) ||
      (open_output != nullptr) != (open_identity != nullptr)) {
    if (error) *error = "incomplete root-open request";
    return false;
  }
  NamespaceOperation operation(configured_->registry());
  if (!operation) {
    if (error) *error = "cannot enter linker operation";
    return false;
  }
  LinkerPublication* raw = nullptr;
  if (!Publish(graph, global, error, &raw)) return false;
  std::unique_ptr<LinkerPublication, decltype(&darwin_art_linker_publication_destroy)> token(
      raw, darwin_art_linker_publication_destroy);
  LinkerImageLease* opened = nullptr;
  if (open_output && (FindResidentFile(open_namespace, *open_identity, &opened, false) != 0 ||
      AcquireNamespaceOpen(&opened, error, nodelete) != 0)) {
    darwin_art_linker_publication_rollback(configured_->registry(), token.get());
    if (error && error->empty()) *error = "published root missing";
    return false;
  }
  std::unique_ptr<LinkerImageLease, decltype(&darwin_art_linker_image_release)> root(
      opened, darwin_art_linker_image_release);
  char detail[4096]{};
  DarwinArtElfErrorBuffer buffer{detail, sizeof(detail), 0};
  if (darwin_art_elf_graph_initialize(graph.get(), &buffer) == DARWIN_ART_ELF_OK) {
    if (open_output) *open_output = root.release();
    return true;
  }
  // No namespace mutex spans foreign code or resource release callbacks.
  const auto rollback = darwin_art_linker_publication_rollback(configured_->registry(), token.get());
  if (error) {
    *error = detail;
    if (rollback != 0) *error += "; namespace publication rollback failed";
  }
  return false;
}
}
