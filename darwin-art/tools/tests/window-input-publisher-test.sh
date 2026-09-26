#!/bin/sh
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
stage=$(mktemp -d "${TMPDIR:-/tmp}/window-input-publisher.XXXXXX")
trap 'rm -rf -- "$stage"' EXIT
javac -d "$stage" "$root"/tools/tests/wm/stubs/android/graphics/Rect.java \
 "$root"/tools/tests/wm/stubs/android/view/InputChannel.java \
 "$root"/tools/tests/wm/stubs/android/view/View.java \
 "$root"/tools/tests/wm/stubs/android/view/WindowManager.java \
 "$root/runtime/framework/wm/WindowInputPublisher.java" \
 "$root/runtime/framework/wm/WindowFocusRegistry.java" \
 "$root/runtime/framework/wm/WindowFocusPublicationDelivery.java" \
 "$root/tools/tests/wm/WindowInputPublisherTest.java"
java -cp "$stage" dev.darwinart.runtime.wm.WindowInputPublisherTest
