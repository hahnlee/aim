#include "android_varargs.h"

#include <cstddef>
#include <cstdint>
#include <cstring>

namespace darwin_art::jni {
namespace {

enum class ValueKind {
  kObject,
  kBoolean,
  kByte,
  kChar,
  kShort,
  kInt,
  kLong,
  kFloat,
  kDouble,
};

bool IsPrimitive(char type) {
  switch (type) {
    case 'Z':
    case 'B':
    case 'C':
    case 'S':
    case 'I':
    case 'J':
    case 'F':
    case 'D':
      return true;
    default:
      return false;
  }
}

ValueKind PrimitiveKind(char type) {
  switch (type) {
    case 'Z':
      return ValueKind::kBoolean;
    case 'B':
      return ValueKind::kByte;
    case 'C':
      return ValueKind::kChar;
    case 'S':
      return ValueKind::kShort;
    case 'I':
      return ValueKind::kInt;
    case 'J':
      return ValueKind::kLong;
    case 'F':
      return ValueKind::kFloat;
    case 'D':
      return ValueKind::kDouble;
    default:
      return ValueKind::kObject;
  }
}

bool ParseObject(const std::string& descriptor, size_t* index) {
  const size_t start = *index;
  while (*index < descriptor.size() && descriptor[*index] != ';') {
    const char character = descriptor[*index];
    // A class name in a descriptor is an internal name. Delimiters and a
    // second descriptor introducer are never part of that name.
    if (character == '[' || character == '(' || character == ')' ||
        character == '.') {
      return false;
    }
    ++*index;
  }
  if (*index >= descriptor.size() || *index == start) {
    return false;
  }
  ++*index;
  return true;
}

bool ParseFieldType(const std::string& descriptor,
                    size_t* index,
                    bool allow_void,
                    ValueKind* kind) {
  if (*index >= descriptor.size()) {
    return false;
  }
  const char type = descriptor[*index];
  if (type == 'V') {
    if (!allow_void) {
      return false;
    }
    ++*index;
    return true;
  }
  if (IsPrimitive(type)) {
    *kind = PrimitiveKind(type);
    ++*index;
    return true;
  }
  if (type == 'L') {
    ++*index;
    if (!ParseObject(descriptor, index)) {
      return false;
    }
    *kind = ValueKind::kObject;
    return true;
  }
  if (type != '[') {
    return false;
  }
  do {
    ++*index;
    if (*index >= descriptor.size()) {
      return false;
    }
  } while (descriptor[*index] == '[');
  // Array components are field types, never void. Objects and all primitive
  // components are represented as one JNI object reference in jvalue.
  ValueKind component = ValueKind::kObject;
  if (!ParseFieldType(descriptor, index, false, &component)) {
    return false;
  }
  *kind = ValueKind::kObject;
  return true;
}

bool ParseDescriptor(const std::string& descriptor,
                     std::vector<ValueKind>* parameters) {
  if (descriptor.empty() || descriptor.front() != '(') {
    return false;
  }
  size_t index = 1;
  while (index < descriptor.size() && descriptor[index] != ')') {
    ValueKind kind = ValueKind::kObject;
    if (!ParseFieldType(descriptor, &index, false, &kind)) {
      return false;
    }
    parameters->push_back(kind);
  }
  if (index >= descriptor.size() || descriptor[index] != ')') {
    return false;
  }
  ++index;
  ValueKind result = ValueKind::kObject;
  if (!ParseFieldType(descriptor, &index, true, &result) ||
      index != descriptor.size()) {
    return false;
  }
  return true;
}

uintptr_t AlignUp(uintptr_t value, uintptr_t alignment) {
  return (value + alignment - 1u) & ~(alignment - 1u);
}

uint64_t ReadGuestGeneral(AndroidArm64VaList* args) {
  const uint8_t* source = nullptr;
  if (args->gr_offs < 0) {
    source = args->gr_top + args->gr_offs;
    args->gr_offs += 8;
  } else {
    args->stack = reinterpret_cast<uint8_t*>(
        AlignUp(reinterpret_cast<uintptr_t>(args->stack), 8));
    source = args->stack;
    args->stack += 8;
  }
  uint64_t value = 0;
  std::memcpy(&value, source, sizeof(value));
  return value;
}

double ReadGuestFloating(AndroidArm64VaList* args) {
  const uint8_t* source = nullptr;
  if (args->vr_offs < 0) {
    source = args->vr_top + args->vr_offs;
    args->vr_offs += 16;
  } else {
    args->stack = reinterpret_cast<uint8_t*>(
        AlignUp(reinterpret_cast<uintptr_t>(args->stack), 8));
    source = args->stack;
    args->stack += 8;
  }
  double value = 0;
  std::memcpy(&value, source, sizeof(value));
  return value;
}

}  // namespace

bool DecodeAndroidArguments(const std::string& descriptor,
                            const void* raw_args,
                            std::vector<jvalue>* output) {
  if (raw_args == nullptr || output == nullptr) {
    return false;
  }
  std::vector<ValueKind> parameters;
  if (!ParseDescriptor(descriptor, &parameters)) {
    return false;
  }

  AndroidArm64VaList args{};
  std::memcpy(&args, raw_args, sizeof(args));
  std::vector<jvalue> decoded;
  decoded.reserve(parameters.size());
  for (const ValueKind kind : parameters) {
    jvalue value{};
    switch (kind) {
      case ValueKind::kObject:
        value.l = reinterpret_cast<jobject>(ReadGuestGeneral(&args));
        break;
      case ValueKind::kBoolean:
        value.z = static_cast<jboolean>(ReadGuestGeneral(&args));
        break;
      case ValueKind::kByte:
        value.b = static_cast<jbyte>(ReadGuestGeneral(&args));
        break;
      case ValueKind::kChar:
        value.c = static_cast<jchar>(ReadGuestGeneral(&args));
        break;
      case ValueKind::kShort:
        value.s = static_cast<jshort>(ReadGuestGeneral(&args));
        break;
      case ValueKind::kInt:
        value.i = static_cast<jint>(ReadGuestGeneral(&args));
        break;
      case ValueKind::kLong:
        value.j = static_cast<jlong>(ReadGuestGeneral(&args));
        break;
      case ValueKind::kFloat:
        value.f = static_cast<jfloat>(ReadGuestFloating(&args));
        break;
      case ValueKind::kDouble:
        value.d = static_cast<jdouble>(ReadGuestFloating(&args));
        break;
    }
    decoded.push_back(value);
  }
  output->swap(decoded);
  return true;
}

}  // namespace darwin_art::jni
