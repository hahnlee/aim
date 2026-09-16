#pragma once
#include "darwin_art_linker_namespace.h"
#include <memory>
#include <string>
#include <vector>
namespace darwin_art::loader {
struct ImageLeaseDrop {
  void operator()(LinkerImageLease* value) const { darwin_art_linker_image_release(value); }
};
using ImageLease = std::unique_ptr<LinkerImageLease, ImageLeaseDrop>;
// Internal close phase. Source must be non-counting, caller holds operation.
// 0 retires handles and consumes source; outgoing exact roots returned for
// subsequent cascade AFTER source resources release. 1 means retained.
// Errors before finalization preserve source. Not a public dlclose by itself.
int ReleaseNamespaceGroup(LinkerRegistry*, ImageLease& source,
    std::vector<ImageLease>* dependencies, std::string* error);
}
