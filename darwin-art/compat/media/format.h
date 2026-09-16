#pragma once

namespace darwin_art::media {
// NDK format functions/data only; codec and image-reader ownership is separate.
void* FormatSymbol(const char* symbol);
}
