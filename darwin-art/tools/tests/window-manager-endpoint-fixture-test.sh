#!/bin/bash
set -euo pipefail

repo_dir=$(cd "$(dirname "$0")/../.." && pwd)
test_dir="$repo_dir/tools/tests/wms-session"
out_dir=$(mktemp -d "${TMPDIR:-/tmp}/darwin-art-wms-session.XXXXXX")
trap 'rm -rf "$out_dir"' EXIT

stubs=()
while IFS= read -r stub; do
  stubs+=("$stub")
done < <(rg --files "$test_dir/stubs" -g '*.java' | sort)
javac --release 8 -encoding UTF-8 -d "$out_dir" \
  "${stubs[@]}" \
  "$repo_dir/tools/tests/wm/unsupported-transactions-stub/dev/darwinart/runtime/os/UnsupportedTransactions.java" \
  "$repo_dir/runtime/framework/am/ApplicationProcessRegistry.java" \
  "$repo_dir/runtime/framework/wm/WindowSessionIdentity.java" \
  "$repo_dir/runtime/framework/wm/DesktopRootWindowBindingOwner.java" \
  "$repo_dir/runtime/framework/wm/DesktopRootRegistry.java" \
  "$repo_dir/runtime/framework/wm/DesktopForegroundAuthority.java" \
  "$repo_dir/runtime/framework/wm/DesktopRootEndpoint.java" \
  "$repo_dir/runtime/framework/wm/DesktopRootProtocol.java" \
  "$repo_dir/runtime/framework/wm/WindowSessionWindowOwnership.java" \
  "$repo_dir/runtime/framework/wm/WindowFocusRegistry.java" \
  "$repo_dir/runtime/framework/wm/WindowRootActivationPolicy.java" \
  "$repo_dir/runtime/framework/wm/DesktopRootFocusDecision.java" \
  "$repo_dir/runtime/framework/wm/DesktopRootFocusDecisionTransport.java" \
  "$repo_dir/runtime/framework/wm/WindowRootFocusDecisionDelivery.java" \
  "$repo_dir/runtime/framework/wm/WindowRootFocusDecisions.java" \
  "$repo_dir/runtime/framework/wm/WindowFocusPublicationDelivery.java" \
  "$repo_dir/runtime/framework/wm/WindowInputEndpoint.java" \
  "$repo_dir/runtime/framework/wm/WindowPublicationDriver.java" \
  "$repo_dir/runtime/framework/wm/WindowPublicationController.java" \
  "$repo_dir/runtime/framework/display/DisplayGeometry.java" \
  "$repo_dir/runtime/framework/display/HostDisplayFacts.java" \
  "$repo_dir/runtime/framework/display/TaskDisplayRegistry.java" \
  "$repo_dir/runtime/framework/wm/WindowSurfaceRegistry.java" \
  "$repo_dir/runtime/framework/wm/WindowSessionEndpoint.java" \
  "$repo_dir/runtime/framework/wm/WindowManagerEndpoint.java" \
  "$test_dir/WindowManagerEndpointFixtureTest.java" \
  "$test_dir/DesktopRootWindowBindingTest.java" \
  "$test_dir/DesktopRootActivationTest.java" \
  "$test_dir/DesktopRootFocusDecisionTest.java"
java -cp "$out_dir" dev.darwinart.runtime.wm.WindowManagerEndpointFixtureTest
java -cp "$out_dir" dev.darwinart.runtime.wm.DesktopRootWindowBindingTest
java -cp "$out_dir" dev.darwinart.runtime.wm.DesktopRootActivationTest
java -cp "$out_dir" dev.darwinart.runtime.wm.DesktopRootFocusDecisionTest
