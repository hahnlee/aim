#include "com_android_apex.h"
#include <cassert>
#include <sstream>

int main() {
  using namespace com::android::apex;
  // Test-only schema fixture, not an installed runtime's APEX inventory.
  auto parsed = parseApexInfoList(R"(<apex-info-list><apex-info
    moduleName="com.android.i18n" modulePath="/apex/com.android.i18n"
    versionCode="1" versionName="1" isFactory="true" isActive="true"
    provideSharedApexLibs="false" partition="SYSTEM"/></apex-info-list>)");
  assert(parsed && parsed->getApexInfo().size() == 1);
  const auto& info = parsed->getApexInfo().front();
  assert(info.getModuleName() == "com.android.i18n");
  assert(info.getVersionCode() == 1 && info.getIsActive());
  assert(info.getPartition() == "SYSTEM" && !info.getProvideSharedApexLibs());
  std::ostringstream serialized;
  write(serialized, *parsed);
  auto roundtrip = parseApexInfoList(serialized.str().c_str());
  assert(roundtrip && roundtrip->getApexInfo().front().getModuleName() == info.getModuleName());
  assert(!parseApexInfoList("<apex-info-list>"));
  std::cout << "AOSP xsdc ApexInfoList XML parse/write/error PASS\n";
}
