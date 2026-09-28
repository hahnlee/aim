#pragma once

namespace aim::media {

// Resolves only the NDK ImageReader/Image symbols owned by this module. The
// media resolver remains responsible for codec/extractor symbols.
void* ImageReaderSymbol(const char* symbol);

}  // namespace aim::media
