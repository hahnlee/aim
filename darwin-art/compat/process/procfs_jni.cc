#include "procfs_jni.h"

#include "../../tools/bionic-fs-facade/include/darwin_art_bionic_fs.h"

#include <stdint.h>

#include <algorithm>
#include <cctype>
#include <cstdlib>
#include <cstring>
#include <string>
#include <vector>

namespace darwin_art::process {
namespace {

void Throw(JNIEnv* env, const char* type, const char* message) {
  if (env == nullptr || env->ExceptionCheck()) return;
  jclass klass = env->FindClass(type);
  if (klass != nullptr) {
    env->ThrowNew(klass, message);
    env->DeleteLocalRef(klass);
  }
}

void ThrowNullPointer(JNIEnv* env, const char* message) {
  Throw(env, "java/lang/NullPointerException", message);
}

void ThrowIllegalArgument(JNIEnv* env, const char* message) {
  Throw(env, "java/lang/IllegalArgumentException", message);
}

enum ProcFormat : jint {
  kProcTermMask = 0xff,
  kProcCombine = 0x100,
  kProcParens = 0x200,
  kProcQuotes = 0x400,
  kProcChar = 0x800,
  kProcOutString = 0x1000,
  kProcOutLong = 0x2000,
  kProcOutFloat = 0x4000,
};

jboolean ParseProcLineArray(JNIEnv* env, char* buffer, jsize start_index,
                            jsize end_index, jintArray format,
                            jobjectArray out_strings, jlongArray out_longs,
                            jfloatArray out_floats) {
  if (buffer == nullptr || format == nullptr || start_index < 0 ||
      end_index < start_index) {
    return JNI_FALSE;
  }

  const jsize format_count = env->GetArrayLength(format);
  const jsize string_count =
      out_strings == nullptr ? 0 : env->GetArrayLength(out_strings);
  const jsize long_count =
      out_longs == nullptr ? 0 : env->GetArrayLength(out_longs);
  const jsize float_count =
      out_floats == nullptr ? 0 : env->GetArrayLength(out_floats);

  jint* format_data = env->GetIntArrayElements(format, nullptr);
  jlong* long_data = out_longs == nullptr
                         ? nullptr
                         : env->GetLongArrayElements(out_longs, nullptr);
  jfloat* float_data = out_floats == nullptr
                           ? nullptr
                           : env->GetFloatArrayElements(out_floats, nullptr);
  if (format_data == nullptr || (long_count > 0 && long_data == nullptr) ||
      (float_count > 0 && float_data == nullptr)) {
    if (format_data != nullptr) {
      env->ReleaseIntArrayElements(format, format_data, 0);
    }
    if (long_data != nullptr) {
      env->ReleaseLongArrayElements(out_longs, long_data, 0);
    }
    if (float_data != nullptr) {
      env->ReleaseFloatArrayElements(out_floats, float_data, 0);
    }
    Throw(env, "java/lang/OutOfMemoryError", nullptr);
    return JNI_FALSE;
  }

  jsize cursor = start_index;
  jsize destination = 0;
  jboolean result = JNI_TRUE;
  for (jsize field = 0; field < format_count; ++field) {
    jint mode = format_data[field];
    if (cursor >= end_index) {
      result = JNI_FALSE;
      break;
    }
    if ((mode & kProcParens) != 0) {
      ++cursor;
    } else if ((mode & kProcQuotes) != 0) {
      if (buffer[cursor] == '"') {
        ++cursor;
      } else {
        mode &= ~kProcQuotes;
      }
    }
    if (cursor > end_index) {
      result = JNI_FALSE;
      break;
    }

    const char terminator = static_cast<char>(mode & kProcTermMask);
    const jsize start = cursor;
    jsize end = -1;
    if ((mode & kProcParens) != 0) {
      while (cursor < end_index && buffer[cursor] != ')') ++cursor;
      end = cursor;
      if (cursor < end_index) ++cursor;
    } else if ((mode & kProcQuotes) != 0) {
      while (cursor < end_index && buffer[cursor] != '"') ++cursor;
      end = cursor;
      if (cursor < end_index) ++cursor;
    }
    while (cursor < end_index && buffer[cursor] != terminator) ++cursor;
    if (end < 0) end = cursor;
    if (cursor < end_index) {
      ++cursor;
      if ((mode & kProcCombine) != 0) {
        while (cursor < end_index && buffer[cursor] == terminator) ++cursor;
      }
    }

    if ((mode & (kProcOutFloat | kProcOutLong | kProcOutString)) == 0) {
      continue;
    }
    const char saved = buffer[end];
    buffer[end] = 0;
    if ((mode & kProcOutFloat) != 0 && destination < float_count) {
      float_data[destination] = std::strtof(buffer + start, nullptr);
    }
    if ((mode & kProcOutLong) != 0 && destination < long_count) {
      long_data[destination] = (mode & kProcChar) != 0
                                   ? buffer[start]
                                   : std::strtoll(buffer + start, nullptr, 10);
    }
    if ((mode & kProcOutString) != 0 && destination < string_count) {
      std::replace_if(buffer + start, buffer + end,
                      [](unsigned char value) { return !std::isprint(value); },
                      '?');
      jstring value = env->NewStringUTF(buffer + start);
      if (value != nullptr) {
        env->SetObjectArrayElement(out_strings, destination, value);
        env->DeleteLocalRef(value);
      }
    }
    buffer[end] = saved;
    ++destination;
  }

  env->ReleaseIntArrayElements(format, format_data, 0);
  if (long_data != nullptr) {
    env->ReleaseLongArrayElements(out_longs, long_data, 0);
  }
  if (float_data != nullptr) {
    env->ReleaseFloatArrayElements(out_floats, float_data, 0);
  }
  return result;
}

}  // namespace

void ReadProcLines(JNIEnv* env, jobject, jstring file_str,
                   jobjectArray req_fields, jlongArray out_fields) {
  // This mirrors Android 16's android_os_Process_readProcLines contract:
  // callers initialize output values to -1, and an unavailable proc file must
  // leave those sentinels untouched.
  if (env == nullptr) return;
  if (file_str == nullptr || req_fields == nullptr || out_fields == nullptr) {
    ThrowNullPointer(env, nullptr);
    return;
  }

  const char* file8 = env->GetStringUTFChars(file_str, nullptr);
  if (file8 == nullptr) return;
  const std::string file(file8);
  env->ReleaseStringUTFChars(file_str, file8);

  const jsize count = env->GetArrayLength(req_fields);
  if (count > env->GetArrayLength(out_fields)) {
    ThrowIllegalArgument(env, "Array lengths differ");
    return;
  }

  std::vector<std::string> fields;
  fields.reserve(static_cast<size_t>(count));
  for (jsize i = 0; i < count; ++i) {
    jobject obj = env->GetObjectArrayElement(req_fields, i);
    if (obj == nullptr) {
      ThrowNullPointer(env, "Element in reqFields");
      return;
    }
    const char* field8 =
        env->GetStringUTFChars(reinterpret_cast<jstring>(obj), nullptr);
    if (field8 == nullptr) {
      ThrowNullPointer(env, "Element in reqFields");
      return;
    }
    fields.emplace_back(field8);
    env->ReleaseStringUTFChars(reinterpret_cast<jstring>(obj), field8);
  }

  jlong* sizes = env->GetLongArrayElements(out_fields, nullptr);
  if (sizes == nullptr) return;

  // darwin_art_bionic_open is a guest ABI boundary.  Passing Darwin's
  // O_CLOEXEC (0x01000000) makes the Android facade reject the open; Bionic's
  // value is 0x00080000 and is published by the facade header.
  constexpr int kOpenFlags = DARWIN_ART_ANDROID_O_CLOEXEC;
  const int fd = darwin_art_bionic_open(file.c_str(), kOpenFlags, 0);
  if (fd >= 0) {
    for (jsize i = 0; i < count; ++i) sizes[i] = 0;

    constexpr size_t kBufferSize = 4096;
    char buffer[kBufferSize];
    const intptr_t read_result =
        darwin_art_bionic_read(fd, buffer, kBufferSize - 1);
    (void)darwin_art_bionic_close(fd);

    int len = 0;
    if (read_result >= 0) {
      len = read_result > static_cast<intptr_t>(kBufferSize - 1)
                ? static_cast<int>(kBufferSize - 1)
                : static_cast<int>(read_result);
    }
    buffer[len] = 0;

    int found_count = 0;
    char* p = buffer;
    while (*p != 0 && found_count < count) {
      bool skip_to_eol = true;
      for (jsize i = 0; i < count; ++i) {
        const std::string& field = fields[static_cast<size_t>(i)];
        if (std::strncmp(p, field.c_str(), field.length()) == 0) {
          p += field.length();
          while (*p == ' ' || *p == '\t') ++p;
          char* number = p;
          while (*p >= '0' && *p <= '9') ++p;
          skip_to_eol = *p != '\n';
          if (*p != 0) {
            *p = 0;
            ++p;
          }
          char* end = nullptr;
          sizes[i] = std::strtoll(number, &end, 10);
          ++found_count;
          break;
        }
      }
      if (skip_to_eol) {
        while (*p != 0 && *p != '\n') ++p;
        if (*p == '\n') ++p;
      }
    }
  }

  env->ReleaseLongArrayElements(out_fields, sizes, 0);
}

jboolean ReadProcFile(JNIEnv* env, jobject, jstring file_str, jintArray format,
                      jobjectArray out_strings, jlongArray out_longs,
                      jfloatArray out_floats) {
  if (env == nullptr) return JNI_FALSE;
  if (file_str == nullptr || format == nullptr) {
    ThrowNullPointer(env, nullptr);
    return JNI_FALSE;
  }

  const char* file_chars = env->GetStringUTFChars(file_str, nullptr);
  if (file_chars == nullptr) {
    Throw(env, "java/lang/OutOfMemoryError", nullptr);
    return JNI_FALSE;
  }
  const std::string file(file_chars);
  env->ReleaseStringUTFChars(file_str, file_chars);

  constexpr int kOpenFlags = DARWIN_ART_ANDROID_O_CLOEXEC;
  const int fd = darwin_art_bionic_open(file.c_str(), kOpenFlags, 0);
  if (fd < 0) return JNI_FALSE;

  constexpr size_t kInitialSize = 1024;
  constexpr size_t kMinGrowthSize = 4096;
  constexpr size_t kMaximumSize = 64U << 20;
  std::vector<char> buffer(kInitialSize);
  size_t offset = 0;
  bool success = true;
  for (;;) {
    const size_t remaining = buffer.size() - offset;
    const intptr_t count =
        darwin_art_bionic_pread(fd, buffer.data() + offset, remaining,
                                static_cast<int64_t>(offset));
    if (count < 0) {
      success = false;
      break;
    }
    offset += static_cast<size_t>(count);
    if (count == 0) break;
    if (offset != buffer.size()) continue;
    if (buffer.size() >= kMaximumSize) {
      success = false;
      break;
    }
    const size_t next_size =
        buffer.size() < kMinGrowthSize ? kMinGrowthSize : buffer.size() * 2;
    buffer.resize(std::min(next_size, kMaximumSize));
  }
  (void)darwin_art_bionic_close(fd);
  if (!success) return JNI_FALSE;
  return ParseProcLineArray(env, buffer.data(), 0, static_cast<jsize>(offset),
                            format, out_strings, out_longs, out_floats);
}

jboolean ParseProcLine(JNIEnv* env, jobject, jbyteArray buffer,
                       jint start_index, jint end_index, jintArray format,
                       jobjectArray out_strings, jlongArray out_longs,
                       jfloatArray out_floats) {
  if (env == nullptr || buffer == nullptr || format == nullptr) {
    if (env != nullptr) ThrowNullPointer(env, nullptr);
    return JNI_FALSE;
  }
  const jsize length = env->GetArrayLength(buffer);
  if (start_index < 0 || end_index < start_index || end_index > length) {
    ThrowIllegalArgument(env, "Invalid proc buffer bounds");
    return JNI_FALSE;
  }
  jbyte* bytes = env->GetByteArrayElements(buffer, nullptr);
  if (bytes == nullptr) return JNI_FALSE;
  const jboolean result = ParseProcLineArray(
      env, reinterpret_cast<char*>(bytes), start_index, end_index, format,
      out_strings, out_longs, out_floats);
  env->ReleaseByteArrayElements(buffer, bytes, 0);
  return result;
}

}  // namespace darwin_art::process
