#include <cstdlib>
#include <iostream>
#include <string>

#include <androidicuinit/android_icu_init.h>
#include <unicode/icudataver.h>
#include <unicode/uclean.h>
#include <unicode/uchar.h>
#include <unicode/ucnv.h>
#include <unicode/uversion.h>

#include "darwin_icu_natives.h"
#include "darwin_libcore_natives.h"

namespace {

std::string VersionString(const uint8_t* version) {
    char text[U_MAX_VERSION_STRING_LENGTH] = {};
    u_versionToString(version, text);
    return text;
}

}  // namespace

int main() {
    android_icu_init();

    UVersionInfo library_version = {};
    UVersionInfo data_version = {};
    u_getVersion(library_version);
    UErrorCode status = U_ZERO_ERROR;
    u_getDataVersion(data_version, &status);
    if (U_FAILURE(status)) {
        std::cerr << "u_getDataVersion failed: " << u_errorName(status) << '\n';
        return 1;
    }

    status = U_ZERO_ERROR;
    UConverter* converter = ucnv_open("UTF-8", &status);
    if (U_FAILURE(status) || converter == nullptr) {
        std::cerr << "ucnv_open failed: " << u_errorName(status) << '\n';
        return 2;
    }
    const char* converter_name = ucnv_getName(converter, &status);

    char character_name[128] = {};
    status = U_ZERO_ERROR;
    const int32_t name_length =
            u_charName(0xAC00, U_UNICODE_CHAR_NAME, character_name,
                       sizeof(character_name), &status);

    const auto charset_registration = &aim::RegisterIcuCharsetNatives;
    const auto libcore_registration = &aim::RegisterLibcoreNatives;
    const bool valid = library_version[0] == 76 && data_version[0] == 76 &&
                       U_SUCCESS(status) && name_length > 0 &&
                       converter_name != nullptr && charset_registration != nullptr &&
                       libcore_registration != nullptr;

    std::cout << "ICU runtime adapters: library=" << VersionString(library_version)
              << " data=" << VersionString(data_version)
              << " converter=" << (converter_name == nullptr ? "?" : converter_name)
              << " character=" << character_name << '\n';

    ucnv_close(converter);
    u_cleanup();
    android_icu_cleanup();
    return valid ? 0 : 3;
}
