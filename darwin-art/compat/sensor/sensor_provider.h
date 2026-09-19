#pragma once

#include <jni.h>

#include <cstddef>

namespace darwin_art::sensor {

// The native manager is process-owned and is shared by the Java framework and
// the NDK entry points.  The opaque pointer is never handed to Java as a
// capability for any other subsystem.
void* ManagerIdentity();
bool IsManagerIdentity(jlong handle);

// Number of host sensors that have a complete Android type/value/event
// mapping.  An observed HID device is not advertised until that mapping and
// event lifetime are implemented.
std::size_t MappedSensorCount();

}  // namespace darwin_art::sensor
