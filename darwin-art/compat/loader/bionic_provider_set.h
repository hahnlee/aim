#pragma once
#include "darwin_art_bionic_provider_namespace.h"
#include <memory>
#include <string>

namespace darwin_art::loader {
struct BionicProviderDrop {
  void operator()(DarwinArtBionicNamespace*) const;
};
using BionicProviderSet = std::unique_ptr<DarwinArtBionicNamespace, BionicProviderDrop>;
// Owns setup until caller transfers it into the Rust runtime native owner.
// A failed bind/seal never publishes a partial provider set. This is the
// existing ported Bionic implementation, not a raw Android libc image loader.
BionicProviderSet CreateBionicProviderSet(std::string* error);
}
