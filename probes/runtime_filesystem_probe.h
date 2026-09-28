#pragma once

// Installs the optional Android system-root facade used by the host probe.
// The implementation is a separate TU so filesystem ownership does not live
// in the ART/HWUI probe translation unit.
bool InstallProbeAndroidSystemRoot();
