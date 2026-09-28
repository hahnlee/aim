#!/bin/bash
# Process boundary: a shared Android system process must not inherit an APK's
# native namespace, resources, providers, receivers or package metadata.
# The caller supplies system-owned launch inputs explicitly as env arguments.
aim_system_service_environment() (
  local name
  for name in ${!AIM_APK_@} ${!AIM_APP_@}; do
    unset "$name"
  done
  # Screenshot/log destinations belong to the acceptance harness, never to
  # the stable Android system runtime configuration.
  unset AIM_ACCEPTANCE_OUTPUT
  exec env "$@"
)
