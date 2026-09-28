#pragma once

namespace aim::media {
// NDK format functions/data only; codec and image-reader ownership is separate.
void* FormatSymbol(const char* symbol);
}
