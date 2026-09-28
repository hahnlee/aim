#include <cstdint>
#include <cstdio>
#include <cstring>

#include "client_utils/android/BitmapRegionDecoder.h"
#include "client_utils/android/FrontBufferedStream.h"
#include "include/core/SkFontMgr.h"
#include "include/ports/SkFontMgr_empty.h"
#include "tools/SkSharingProc.h"

int main() {
    sk_sp<SkFontMgr> font_manager = SkFontMgr_New_Custom_Empty();
    SkSharingSerialContext sharing_context{};
    sharing_context.setDirectContext(nullptr);

    const auto serialize = &SkSharingSerialContext::serializeImage;
    const char bytes[] = "abcdef";
    auto source = std::make_unique<SkMemoryStream>(bytes, sizeof(bytes) - 1, true);
    auto buffered = android::skia::FrontBufferedStream::Make(std::move(source), 3);
    char first[3]{};
    char replay[3]{};
    const bool buffered_ok = buffered && buffered->read(first, sizeof(first)) == sizeof(first) &&
                             buffered->rewind() &&
                             buffered->read(replay, sizeof(replay)) == sizeof(replay) &&
                             std::memcmp(first, replay, sizeof(first)) == 0;
    auto invalid_decoder = android::skia::BitmapRegionDecoder::Make(
            SkData::MakeWithCopy("not-an-image", 12));
    const bool android_utils_ok = buffered_ok && !invalid_decoder;
    const bool ok = font_manager && font_manager->countFamilies() == 1 && serialize != nullptr &&
                    android_utils_ok;
    std::printf("Skia HWUI force-load: emptyFamilies=%d sharing=%s androidUtils=%s\n",
                font_manager ? font_manager->countFamilies() : -1,
                serialize ? "yes" : "no", android_utils_ok ? "yes" : "no");
    return ok ? 0 : 1;
}
