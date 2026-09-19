#include "sensor_provider.h"

#include <android/looper.h>
#include <android/sensor.h>

#include <cerrno>
#include <cstddef>
#include <cstdint>
#include <limits>

// Keep these opaque Android NDK handles owned by this subsystem.  The shared
// manager identity is also used by SystemSensorManager JNI, so an arbitrary
// pointer can never be accepted as an Android sensor capability.
struct ASensorManager {
  std::uint32_t magic;
};

namespace darwin_art::sensor {
namespace {

constexpr std::uint32_t kManagerMagic = 0x53454e4du;  // "SENM"

ASensorManager g_manager{kManagerMagic};

bool IsManager(const ASensorManager* manager) {
  return manager == &g_manager && manager->magic == kManagerMagic;
}

}  // namespace

void* ManagerIdentity() {
  return &g_manager;
}

bool IsManagerIdentity(jlong handle) {
  return IsManager(reinterpret_cast<const ASensorManager*>(handle));
}

std::size_t MappedSensorCount() {
  // No macOS device currently has a complete Android sensor type, value and
  // event-queue mapping.  Keep the inventory empty until a provider can own
  // the full event contract; never report a fabricated sensor.
  return 0;
}

}  // namespace darwin_art::sensor

extern "C" ASensorManager* ASensorManager_getInstanceForPackage(
    const char*) {
  return static_cast<ASensorManager*>(darwin_art::sensor::ManagerIdentity());
}

extern "C" ASensorManager* ASensorManager_getInstance() {
  return static_cast<ASensorManager*>(darwin_art::sensor::ManagerIdentity());
}

extern "C" int ASensorManager_getSensorList(ASensorManager* manager,
                                               ASensorList* list) {
  if (list != nullptr) *list = nullptr;
  if (!darwin_art::sensor::IsManagerIdentity(
          reinterpret_cast<jlong>(manager))) {
    return -EINVAL;
  }
  return 0;
}

extern "C" const ASensor* ASensorManager_getDefaultSensor(ASensorManager* manager,
                                                            int) {
  if (!darwin_art::sensor::IsManagerIdentity(
          reinterpret_cast<jlong>(manager))) {
    return nullptr;
  }
  return nullptr;
}

extern "C" ASensorEventQueue* ASensorManager_createEventQueue(
    ASensorManager* manager, ALooper* looper, int, ALooper_callbackFunc,
    void*) {
  if (!darwin_art::sensor::IsManagerIdentity(
          reinterpret_cast<jlong>(manager)) ||
      looper == nullptr || darwin_art::sensor::MappedSensorCount() == 0) {
    return nullptr;
  }
  // Queue creation will be implemented together with a real host event
  // source.  Returning success without one would turn an unsupported sensor
  // into a silently empty event stream.
  return nullptr;
}

extern "C" int ASensorManager_destroyEventQueue(ASensorManager* manager,
                                                 ASensorEventQueue* queue) {
  if (!darwin_art::sensor::IsManagerIdentity(
          reinterpret_cast<jlong>(manager))) {
    return -EINVAL;
  }
  (void)queue;
  return -EINVAL;
}

extern "C" int ASensorEventQueue_enableSensor(ASensorEventQueue* queue,
                                                const ASensor* sensor) {
  (void)queue;
  (void)sensor;
  return -EINVAL;
}

extern "C" int ASensorEventQueue_disableSensor(ASensorEventQueue* queue,
                                                 const ASensor* sensor) {
  (void)queue;
  (void)sensor;
  return -EINVAL;
}

extern "C" int ASensorEventQueue_setEventRate(ASensorEventQueue* queue,
                                                const ASensor* sensor,
                                                int32_t) {
  (void)queue;
  (void)sensor;
  return -EINVAL;
}

extern "C" ssize_t ASensorEventQueue_getEvents(ASensorEventQueue* queue,
                                                 ASensorEvent*, size_t) {
  (void)queue;
  return -EINVAL;
}

extern "C" int ASensorEventQueue_hasEvents(ASensorEventQueue* queue) {
  (void)queue;
  return -EINVAL;
}

extern "C" const char* ASensor_getName(const ASensor*) { return nullptr; }

extern "C" float ASensor_getResolution(const ASensor*) {
  return std::numeric_limits<float>::quiet_NaN();
}

extern "C" int ASensor_getType(const ASensor*) { return -EINVAL; }

extern "C" const char* ASensor_getVendor(const ASensor*) { return nullptr; }

extern "C" int ASensor_getMinDelay(const ASensor*) { return -EINVAL; }
