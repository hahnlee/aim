#pragma once
#include <cstdio>

namespace darwin_art::graphics {
// Borrow an Android descriptor, return an independently owned Darwin stream.
// The caller transfers the stream to SkFILEStream or closes it on failure.
// Never treat an unknown Android descriptor number as a host descriptor.
FILE* OpenImageDecoderInput(int guest_fd);
}
