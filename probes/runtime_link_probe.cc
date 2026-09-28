// Compatibility include for tools that still refer to the historical probe
// name. Production action graphs compile runtime_entry_probe.cc directly so
// the entrypoint has an independent TU/cache boundary.
#include "runtime_entry_probe.cc"
