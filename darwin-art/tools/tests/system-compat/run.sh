#!/bin/bash
set -euo pipefail
root="$(cd "$(dirname "$0")/../../.." && pwd)"
sdk="${ANDROID_SDK_ROOT:-${ANDROID_HOME:-$HOME/Library/Android/sdk}}"
parser="$sdk/cmdline-tools/latest/lib/external/net/sf/kxml/kxml2/2.3.0/kxml2-2.3.0.jar"
[[ -f "$parser" && ! -L "$parser" ]] || { echo 'installed SDK KXml parser required' >&2; exit 69; }
stage="$(mktemp -d /tmp/darwin-art-system-compat-input.XXXXXX)"
trap 'rm -rf -- "$stage"' EXIT
javac --release 8 -cp "$parser" -d "$stage/classes" \
  "$root/tools/tests/system-compat/Xml.java" \
  "$root/tools/tests/system-compat/SystemCompatInputTest.java" \
  "$root/runtime/framework/compile-stubs/com/android/server/compat/config/Config.java" \
  "$root/runtime/framework/compile-stubs/com/android/server/compat/config/Change.java" \
  "$root/runtime/framework/compile-stubs/com/android/server/compat/config/XmlParser.java" \
  "$root/runtime/framework/compat/SystemCompatCatalog.java" \
  "$root/runtime/framework/compat/SystemCompatConfigReader.java"
java -ea -cp "$stage/classes:$parser" dev.darwinart.runtime.compat.SystemCompatInputTest "$stage/tree"
javac --release 8 -d "$stage/admission" \
  "$root/tools/tests/system-compat/Binder.java" \
  "$root/tools/tests/system-compat/Process.java" \
  "$root/tools/tests/system-compat/ServiceDirectoryAdmissionTest.java" \
  "$root/runtime/framework/system/ServiceDirectoryAdmission.java"
java -ea -cp "$stage/admission" dev.darwinart.runtime.system.ServiceDirectoryAdmissionTest
