#!/bin/bash
# Process boundary: a shared Android system process must not inherit an APK's
# native namespace, resources, providers, receivers or package metadata.
# The caller supplies system-owned launch inputs explicitly as env arguments.
darwin_art_system_service_environment() (
  local name
  for name in ${!DARWIN_ART_APK_@} ${!DARWIN_ART_APP_@}; do
    unset "$name"
  done
  exec env "$@"
)
