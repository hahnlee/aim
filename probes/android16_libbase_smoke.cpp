#include <cerrno>
#include <cstdio>
#include <cstring>
#include <string>
#include <unistd.h>

#include <android-base/errors.h>
#include <android-base/file.h>
#include <android-base/format.h>
#include <android-base/logging.h>
#include <android-base/parseint.h>
#include <android-base/properties.h>
#include <android-base/strings.h>
#include <android-base/test_utils.h>

extern "C" int posix_strerror_r(int, char*, size_t);

int main() {
  int parsed = 0;
  if (!android::base::ParseInt("42", &parsed) || parsed != 42) return 10;
  const auto joined = android::base::Join(android::base::Split("base:darwin", ":"), "/");
  if (joined != "base/darwin") return 11;
  if (!android::base::SetProperty("AIM_LIBBASE_SMOKE", "ready")) return 12;
  if (android::base::GetProperty("AIM_LIBBASE_SMOKE", "") != "ready") return 13;

  TemporaryFile temporary;
  const std::string payload = fmt::format("{}:{}", joined, parsed);
  if (!android::base::WriteStringToFd(payload, temporary.fd)) return 14;
  if (lseek(temporary.fd, 0, SEEK_SET) == -1) return 15;
  std::string restored;
  if (!android::base::ReadFdToString(temporary.fd, &restored) || restored != payload) return 16;

  char error[128]{};
  if (posix_strerror_r(EINVAL, error, sizeof(error)) != 0 || error[0] == '\0') return 17;
  if (android::base::SystemErrorCodeToString(EINVAL).empty()) return 18;
  const auto previous = android::base::SetMinimumLogSeverity(android::base::WARNING);
  android::base::SetMinimumLogSeverity(previous);
  std::printf("libbase-smoke: payload=%s errno=%s\n", payload.c_str(), error);
  return 0;
}
