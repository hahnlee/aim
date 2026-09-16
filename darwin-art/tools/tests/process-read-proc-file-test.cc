#include "../../compat/process/procfs_jni.h"
#include "../bionic-fs-facade/include/darwin_art_bionic_fs.h"

#include <cassert>
#include <cstring>
#include <fcntl.h>
#include <memory>
#include <string>
#include <vector>

namespace {

enum class Kind { kString, kStrings, kLongs, kInts };
struct Header {
  Kind kind;
};
struct String : Header {
  std::string value;
};
struct Strings : Header {
  std::vector<jstring> values;
};
struct Longs : Header {
  std::vector<jlong> values;
};
struct Ints : Header {
  std::vector<jint> values;
};

bool pending = false;
bool proc_exists = true;
std::string proc_contents;
std::vector<std::unique_ptr<String>> created_strings;
int open_calls = 0;
int close_calls = 0;

String path{{Kind::kString}, "/proc/self/stat"};
Ints format{{Kind::kInts},
            {0x2000 | ' ', 0x1000 | 0x200 | ' ',
             0x2000 | 0x800 | ' '}};
Strings strings{{Kind::kStrings}, {nullptr, nullptr, nullptr}};
Longs longs{{Kind::kLongs}, {-1, -1, -1}};

const char* GetStringUTFChars(JNIEnv*, jstring value, jboolean*) {
  return reinterpret_cast<String*>(value)->value.c_str();
}
void ReleaseStringUTFChars(JNIEnv*, jstring, const char*) {}
jsize GetArrayLength(JNIEnv*, jarray value) {
  Header* header = reinterpret_cast<Header*>(value);
  switch (header->kind) {
    case Kind::kStrings:
      return static_cast<jsize>(
          reinterpret_cast<Strings*>(value)->values.size());
    case Kind::kLongs:
      return static_cast<jsize>(reinterpret_cast<Longs*>(value)->values.size());
    case Kind::kInts:
      return static_cast<jsize>(reinterpret_cast<Ints*>(value)->values.size());
    case Kind::kString:
      return 0;
  }
}
jint* GetIntArrayElements(JNIEnv*, jintArray value, jboolean*) {
  return reinterpret_cast<Ints*>(value)->values.data();
}
void ReleaseIntArrayElements(JNIEnv*, jintArray, jint*, jint) {}
jlong* GetLongArrayElements(JNIEnv*, jlongArray value, jboolean*) {
  return reinterpret_cast<Longs*>(value)->values.data();
}
void ReleaseLongArrayElements(JNIEnv*, jlongArray, jlong*, jint) {}
jstring NewStringUTF(JNIEnv*, const char* value) {
  created_strings.push_back(
      std::make_unique<String>(String{{Kind::kString}, value}));
  return reinterpret_cast<jstring>(created_strings.back().get());
}
void SetObjectArrayElement(JNIEnv*, jobjectArray array, jsize index,
                           jobject value) {
  reinterpret_cast<Strings*>(array)->values.at(static_cast<size_t>(index)) =
      reinterpret_cast<jstring>(value);
}
jclass FindClass(JNIEnv*, const char*) { return reinterpret_cast<jclass>(&path); }
jint ThrowNew(JNIEnv*, jclass, const char*) {
  pending = true;
  return JNI_OK;
}
jboolean ExceptionCheck(JNIEnv*) { return pending ? JNI_TRUE : JNI_FALSE; }
void DeleteLocalRef(JNIEnv*, jobject) {}

}  // namespace

extern "C" int darwin_art_bionic_open(const char* value, int flags,
                                       uint32_t) {
  assert(std::strcmp(value, "/proc/self/stat") == 0);
  assert(flags == DARWIN_ART_ANDROID_O_CLOEXEC);
  ++open_calls;
  return proc_exists ? 42 : -1;
}
extern "C" intptr_t darwin_art_bionic_pread(int fd, void* buffer, size_t count,
                                             int64_t offset) {
  assert(fd == 42);
  if (offset >= static_cast<int64_t>(proc_contents.size())) return 0;
  const size_t available = proc_contents.size() - static_cast<size_t>(offset);
  const size_t copied = std::min(available, count);
  std::memcpy(buffer, proc_contents.data() + offset, copied);
  return static_cast<intptr_t>(copied);
}
extern "C" intptr_t darwin_art_bionic_read(int, void*, size_t) { return -1; }
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
  functions.GetIntArrayElements = GetIntArrayElements;
  functions.ReleaseIntArrayElements = ReleaseIntArrayElements;
  functions.GetLongArrayElements = GetLongArrayElements;
  functions.ReleaseLongArrayElements = ReleaseLongArrayElements;
  functions.NewStringUTF = NewStringUTF;
  functions.SetObjectArrayElement = SetObjectArrayElement;
  functions.FindClass = FindClass;
  functions.ThrowNew = ThrowNew;
  functions.ExceptionCheck = ExceptionCheck;
  functions.DeleteLocalRef = DeleteLocalRef;
  JNIEnv env{&functions};

  proc_contents = "41326 (Chrome Render) S 1 2 3\n";
  const jboolean parsed = darwin_art::process::ReadProcFile(
      &env, nullptr, reinterpret_cast<jstring>(&path),
      reinterpret_cast<jintArray>(&format),
      reinterpret_cast<jobjectArray>(&strings),
      reinterpret_cast<jlongArray>(&longs), nullptr);
  assert(parsed == JNI_TRUE);
  assert(!pending);
  assert(open_calls == 1 && close_calls == 1);
  assert(longs.values[0] == 41326);
  assert(longs.values[2] == 'S');
  assert(reinterpret_cast<String*>(strings.values[1])->value ==
         "Chrome Render");

  proc_exists = false;
  assert(darwin_art::process::ReadProcFile(
             &env, nullptr, reinterpret_cast<jstring>(&path),
             reinterpret_cast<jintArray>(&format),
             reinterpret_cast<jobjectArray>(&strings),
             reinterpret_cast<jlongArray>(&longs), nullptr) == JNI_FALSE);
  assert(open_calls == 2 && close_calls == 1);

  pending = false;
  assert(darwin_art::process::ReadProcFile(
             &env, nullptr, nullptr, reinterpret_cast<jintArray>(&format),
             reinterpret_cast<jobjectArray>(&strings),
             reinterpret_cast<jlongArray>(&longs), nullptr) == JNI_FALSE);
  assert(pending);
  return 0;
}
