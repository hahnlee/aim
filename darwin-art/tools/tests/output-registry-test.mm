#include "../../compat/surfaceflinger/output_registry.h"

#import <CoreFoundation/CoreFoundation.h>
#import <IOSurface/IOSurface.h>

#include <assert.h>
#include <errno.h>

#include <array>
#include <cstdint>
#include <cstdio>
#include <memory>
#include <span>

namespace {

using darwin_art::surfaceflinger::OutputOperation;
using darwin_art::surfaceflinger::OutputRegistry;
using darwin_art::surfaceflinger::OutputRequest;

struct Surface {
  IOSurfaceRef value = nullptr;
  uint32_t id = 0;
  uint32_t width = 0;
  uint32_t height = 0;
};

Surface MakeSurface(uint32_t width, uint32_t height) {
  const int32_t width_value = static_cast<int32_t>(width);
  const int32_t height_value = static_cast<int32_t>(height);
  const int32_t bytes_per_element = 4;
  const int32_t bytes_per_row = width_value * bytes_per_element;
  const int32_t allocation_size = height_value * bytes_per_row;
  CFMutableDictionaryRef properties = CFDictionaryCreateMutable(
      kCFAllocatorDefault, 5, &kCFTypeDictionaryKeyCallBacks,
      &kCFTypeDictionaryValueCallBacks);
  assert(properties != nullptr);
  CFNumberRef width_number =
      CFNumberCreate(kCFAllocatorDefault, kCFNumberSInt32Type, &width_value);
  CFNumberRef height_number =
      CFNumberCreate(kCFAllocatorDefault, kCFNumberSInt32Type, &height_value);
  CFNumberRef bytes_number = CFNumberCreate(
      kCFAllocatorDefault, kCFNumberSInt32Type, &bytes_per_element);
  CFNumberRef row_number =
      CFNumberCreate(kCFAllocatorDefault, kCFNumberSInt32Type, &bytes_per_row);
  CFNumberRef allocation_number = CFNumberCreate(
      kCFAllocatorDefault, kCFNumberSInt32Type, &allocation_size);
  assert(width_number != nullptr && height_number != nullptr &&
         bytes_number != nullptr && row_number != nullptr &&
         allocation_number != nullptr);
  CFDictionarySetValue(properties, kIOSurfaceWidth, width_number);
  CFDictionarySetValue(properties, kIOSurfaceHeight, height_number);
  CFDictionarySetValue(properties, kIOSurfaceBytesPerElement, bytes_number);
  CFDictionarySetValue(properties, kIOSurfaceBytesPerRow, row_number);
  CFDictionarySetValue(properties, kIOSurfaceAllocSize, allocation_number);
  IOSurfaceRef surface = IOSurfaceCreate(properties);
  CFRelease(allocation_number);
  CFRelease(row_number);
  CFRelease(bytes_number);
  CFRelease(height_number);
  CFRelease(width_number);
  CFRelease(properties);
  assert(surface != nullptr);
  return {surface, IOSurfaceGetID(surface), width, height};
}

OutputRequest Request(uint32_t id, uint32_t width, uint32_t height) {
  OutputRequest request{};
  request.iosurface_id = id;
  request.physical_width = width;
  request.physical_height = height;
  request.logical_width = width;
  request.logical_height = height;
  return request;
}

void TestRegistryTransitions() {
  Surface first = MakeSurface(19, 11);
  Surface replacement = MakeSurface(31, 13);
  OutputRegistry registry;

  auto registered = registry.Apply(7, Request(first.id, first.width, first.height));
  assert(registered.response.status == 0 && registered.current != nullptr);
  assert(registered.current->current());
  assert(registered.current->identity().generation == 1);
  assert(registered.current->identity().iosurface_id == first.id);
  auto admitted = registry.Admit(first.id);
  assert(admitted == registered.current);
  auto native = static_cast<IOSurfaceRef>(admitted->backing()->native_surface());
  assert(IOSurfaceGetWidth(native) == first.width &&
         IOSurfaceGetHeight(native) == first.height);

  auto duplicate =
      registry.Apply(8, Request(first.id, first.width, first.height));
  assert(duplicate.response.status == EEXIST);
  assert(registry.Admit(first.id) == registered.current);

  OutputRequest unauthorized =
      Request(first.id, first.width, first.height);
  unauthorized.operation = OutputOperation::Replace;
  unauthorized.token = registered.current->identity().token;
  unauthorized.token.serial++;
  unauthorized.generation = registered.current->identity().generation;
  auto denied = registry.Apply(7, unauthorized);
  assert(denied.response.status == ESTALE && denied.current == nullptr);
  assert(registry.Admit(first.id) == registered.current);
  assert(registered.current->current());

  std::weak_ptr<const darwin_art::surfaceflinger::IosurfaceBacking> old_weak =
      registered.current->backing();
  OutputRequest same_id =
      Request(first.id, first.width, first.height);
  same_id.operation = OutputOperation::Replace;
  same_id.token = registered.current->identity().token;
  same_id.generation = registered.current->identity().generation;
  auto same = registry.Apply(7, same_id);
  assert(same.response.status == 0 && same.current != nullptr &&
         same.previous == registered.current);
  assert(same.current->identity().generation == 2);
  assert(!same.previous->current());
  assert(registry.Admit(first.id) == same.current);
  admitted.reset();
  registered.current.reset();
  assert(!old_weak.expired());
  same.previous.reset();
  assert(old_weak.expired());

  OutputRequest new_id =
      Request(replacement.id, replacement.width, replacement.height);
  new_id.operation = OutputOperation::Replace;
  new_id.token = same.current->identity().token;
  new_id.generation = same.current->identity().generation;
  auto migrated = registry.Apply(7, new_id);
  assert(migrated.response.status == 0 && migrated.current != nullptr);
  assert(migrated.current->identity().generation == 3);
  assert(!migrated.previous->current());
  assert(registry.Admit(first.id) == nullptr);
  assert(registry.Admit(replacement.id) == migrated.current);

  std::weak_ptr<const darwin_art::surfaceflinger::IosurfaceBacking> retained =
      migrated.current->backing();
  auto dropped = registry.Drop(7);
  assert(dropped.response.status == 0 && dropped.current == nullptr &&
         dropped.previous == migrated.current);
  assert(!dropped.previous->current());
  assert(registry.Admit(replacement.id) == nullptr);
  migrated.current.reset();
  assert(!retained.expired());
  dropped.previous.reset();
  assert(retained.expired());
  CFRelease(replacement.value);
  CFRelease(first.value);
}

}  // namespace

int main() {
  TestRegistryTransitions();
  std::puts("output registry: real backing/extents, duplicate and unauthorized "
            "rejection, generation replacement, ID migration, drop invalidation "
            "and retained-epoch release PASS");
}
