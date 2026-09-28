#include <cstdlib>
#include <iostream>
#include <memory>
#include <string>

#include <androidicuinit/android_icu_init.h>
#include <unicode/brkiter.h>
#include <unicode/icudataver.h>
#include <unicode/locid.h>
#include <unicode/numfmt.h>
#include <unicode/uclean.h>
#include <unicode/ustring.h>
#include <unicode/uversion.h>

namespace {

[[noreturn]] void Fail(const char* operation, UErrorCode status) {
  std::cerr << operation << " failed: " << u_errorName(status) << '\n';
  std::exit(1);
}

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
    Fail("u_getDataVersion", status);
  }

  std::string formatted;
  int boundaries = 0;
  {
    status = U_ZERO_ERROR;
    std::unique_ptr<icu::NumberFormat> formatter(
        icu::NumberFormat::createInstance(icu::Locale::getKorean(), status));
    if (U_FAILURE(status) || formatter == nullptr) {
      Fail("NumberFormat::createInstance", status);
    }
    icu::UnicodeString result;
    formatter->format(1234567.5, result);
    result.toUTF8String(formatted);

    status = U_ZERO_ERROR;
    std::unique_ptr<icu::BreakIterator> words(
        icu::BreakIterator::createWordInstance(icu::Locale::getKorean(), status));
    if (U_FAILURE(status) || words == nullptr) {
      Fail("BreakIterator::createWordInstance", status);
    }
    words->setText(icu::UnicodeString::fromUTF8("Android ICU 한글 테스트"));
    for (int32_t boundary = words->first();
         boundary != icu::BreakIterator::DONE;
         boundary = words->next()) {
      ++boundaries;
    }
  }

  if (library_version[0] != 76 || data_version[0] != 76 ||
      formatted.empty() || boundaries < 2) {
    std::cerr << "unexpected ICU result\n";
    return 2;
  }

  std::cout << "icu_version=" << VersionString(library_version)
            << " data_version=" << VersionString(data_version)
            << " formatted=" << formatted
            << " word_boundaries=" << boundaries << '\n';

  u_cleanup();
  android_icu_cleanup();
  return 0;
}
