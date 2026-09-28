#include <iostream>
#include <string>

#include <androidicuinit/android_icu_init.h>
#include <unicode/ubrk.h>
#include <unicode/uclean.h>
#include <unicode/ucnv.h>
#include <unicode/ucol.h>
#include <unicode/icudataver.h>
#include <unicode/ures.h>
#include <unicode/uversion.h>

namespace {

std::string VersionString(const uint8_t* version) {
  char text[U_MAX_VERSION_STRING_LENGTH] = {};
  u_versionToString(version, text);
  return text;
}

void Report(const char* name, UErrorCode status, bool* all_present) {
  const bool present = U_SUCCESS(status);
  std::cout << "resource." << name << '='
            << (present ? "present" : u_errorName(status)) << '\n';
  *all_present &= present;
}

}  // namespace

int main() {
  android_icu_init();

  UVersionInfo library_version = {};
  UVersionInfo data_version = {};
  u_getVersion(library_version);
  UErrorCode data_status = U_ZERO_ERROR;
  u_getDataVersion(data_version, &data_status);

  std::cout << "icu4c.library.version=" << VersionString(library_version) << '\n';
  std::cout << "icu4c.data.version="
            << (U_SUCCESS(data_status) ? VersionString(data_version)
                                       : u_errorName(data_status))
            << '\n';

  bool all_present = U_SUCCESS(data_status);

  UErrorCode status = U_ZERO_ERROR;
  UResourceBundle* version_bundle = ures_open(nullptr, "icuver", &status);
  if (U_SUCCESS(status)) {
    int32_t length = 0;
    (void)ures_getStringByKey(version_bundle, "DataVersion", &length, &status);
    if (U_SUCCESS(status) && length == 0) {
      status = U_MISSING_RESOURCE_ERROR;
    }
  }
  Report("icuver.DataVersion", status, &all_present);
  ures_close(version_bundle);

  status = U_ZERO_ERROR;
  UResourceBundle* root = ures_open(nullptr, "root", &status);
  Report("root_bundle", status, &all_present);
  ures_close(root);

  status = U_ZERO_ERROR;
  UConverter* converter = ucnv_open("UTF-8", &status);
  Report("converter.UTF-8", status, &all_present);
  ucnv_close(converter);

  status = U_ZERO_ERROR;
  UCollator* collator = ucol_open("root", &status);
  Report("collation.root", status, &all_present);
  ucol_close(collator);

  status = U_ZERO_ERROR;
  UBreakIterator* break_iterator =
      ubrk_open(UBRK_WORD, "root", nullptr, 0, &status);
  Report("break_iterator.root", status, &all_present);
  ubrk_close(break_iterator);

  u_cleanup();
  android_icu_cleanup();
  return all_present ? 0 : 3;
}
