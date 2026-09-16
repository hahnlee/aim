#include "../../compat/process/procfs_jni.h"
#include "../bionic-fs-facade/include/darwin_art_bionic_fs.h"

#include <cassert>
#include <cstring>
#include <fcntl.h>
#include <string>
#include <vector>

namespace {

enum Kind { kString, kStrings, kLongs };
struct Header { Kind kind; };
struct String : Header { const char* value; };
struct Strings : Header { std::vector<jstring> values; };
struct Longs : Header { std::vector<jlong> values; };

bool pending = false;
std::string proc_contents;
bool proc_exists = true;
int open_calls = 0;
int close_calls = 0;

String path{{kString}, "/proc/self/status"};
String uid_field{{kString}, "Uid:"};
String rss_anon_field{{kString}, "RssAnon:"};
String rss_shmem_field{{kString}, "RssShmem:"};
Strings fields{{kStrings}, {reinterpret_cast<jstring>(&uid_field),
                            reinterpret_cast<jstring>(&rss_anon_field),
                            reinterpret_cast<jstring>(&rss_shmem_field)}};
Longs outputs{{kLongs}, {-1, -1, -1}};

const char* GetStringUTFChars(JNIEnv*, jstring value, jboolean*) {
  return reinterpret_cast<String*>(value)->value;
}
void ReleaseStringUTFChars(JNIEnv*, jstring, const char*) {}
jsize GetArrayLength(JNIEnv*, jarray value) {
  Header* header = reinterpret_cast<Header*>(value);
  if (header->kind == kStrings) {
    return static_cast<jsize>(reinterpret_cast<Strings*>(value)->values.size());
  }
  return static_cast<jsize>(reinterpret_cast<Longs*>(value)->values.size());
}
jobject GetObjectArrayElement(JNIEnv*, jobjectArray value, jsize index) {
  return reinterpret_cast<Strings*>(value)->values.at(static_cast<size_t>(index));
}
jlong* GetLongArrayElements(JNIEnv*, jlongArray value, jboolean*) {
  return reinterpret_cast<Longs*>(value)->values.data();
}
void ReleaseLongArrayElements(JNIEnv*, jlongArray, jlong*, jint) {}
jclass FindClass(JNIEnv*, const char*) { return reinterpret_cast<jclass>(&path); }
jint ThrowNew(JNIEnv*, jclass, const char*) { pending = true; return JNI_OK; }
jboolean ExceptionCheck(JNIEnv*) { return pending ? JNI_TRUE : JNI_FALSE; }
void DeleteLocalRef(JNIEnv*, jobject) {}

}  // namespace

extern "C" int darwin_art_bionic_open(const char* value, int flags,
                                       uint32_t) {
  assert(std::strcmp(value, "/proc/self/status") == 0);
  assert(flags == DARWIN_ART_ANDROID_O_CLOEXEC);
  ++open_calls;
  return proc_exists ? 42 : -1;
}
extern "C" intptr_t darwin_art_bionic_read(int fd, void* buffer, size_t count) {
  assert(fd == 42);
  const size_t length = proc_contents.size();
  const size_t copied = length < count ? length : count;
  std::memcpy(buffer, proc_contents.data(), copied);
  return static_cast<intptr_t>(copied);
}
extern "C" intptr_t darwin_art_bionic_pread(int, void*, size_t, int64_t) {
  return -1;
}
extern "C" int darwin_art_bionic_close(int fd) {
  assert(fd == 42);
  ++close_calls;
  return 0;
}

int main() {
  JNINativeInterface functions{};
  functions.GetStringUTFChars = GetStringUTFChars;
  functions.ReleaseStringUTFChars = ReleaseStringUTFChars;
  functions.GetArrayLength = GetArrayLength;
  functions.GetObjectArrayElement = GetObjectArrayElement;
  functions.GetLongArrayElements = GetLongArrayElements;
  functions.ReleaseLongArrayElements = ReleaseLongArrayElements;
  functions.FindClass = FindClass;
  functions.ThrowNew = ThrowNew;
  functions.ExceptionCheck = ExceptionCheck;
  functions.DeleteLocalRef = DeleteLocalRef;
  JNIEnv env{&functions};

  proc_contents = "Name: host\nUid: 501 501 501 501\nRssAnon: 123 kB\n"
                  "RssShmem: 0 kB\nVmSwap: 0 kB\n";
  pending = false;
  proc_exists = true;
  open_calls = close_calls = 0;
  outputs.values = {-1, -1, -1};
  darwin_art::process::ReadProcLines(
      &env, nullptr, reinterpret_cast<jstring>(&path),
      reinterpret_cast<jobjectArray>(&fields), reinterpret_cast<jlongArray>(&outputs));
  assert(!pending);
  assert(outputs.values == std::vector<jlong>({501, 123, 0}));
  assert(open_calls == 1 && close_calls == 1);

  pending = false;
  proc_exists = false;
  outputs.values = {-1, -1, -1};
  darwin_art::process::ReadProcLines(
      &env, nullptr, reinterpret_cast<jstring>(&path),
      reinterpret_cast<jobjectArray>(&fields), reinterpret_cast<jlongArray>(&outputs));
  assert(!pending && outputs.values == std::vector<jlong>({-1, -1, -1}));

  pending = false;
  darwin_art::process::ReadProcLines(&env, nullptr, nullptr,
                                     reinterpret_cast<jobjectArray>(&fields),
                                     reinterpret_cast<jlongArray>(&outputs));
  assert(pending);

  pending = false;
  Longs too_short{{kLongs}, {-1}};
  darwin_art::process::ReadProcLines(
      &env, nullptr, reinterpret_cast<jstring>(&path),
      reinterpret_cast<jobjectArray>(&fields), reinterpret_cast<jlongArray>(&too_short));
  assert(pending);
  return 0;
}
