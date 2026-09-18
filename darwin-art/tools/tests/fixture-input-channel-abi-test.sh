#!/bin/zsh
set -euo pipefail
repo_dir="$(cd "$(dirname "$0")/../.." && pwd)"
# The fixture exercises the currently pinned API-36 boot framework, not SDK
# stubs or a copied Java implementation. No product or DEX build is performed.
fixture_sdk_root="${ANDROID_SDK_ROOT:-${ANDROID_HOME:-$HOME/Library/Android/sdk}}"
dexdump_bin="$fixture_sdk_root/build-tools/36.0.0/dexdump"
framework_dex="$repo_dir/_build/bootclasspath/framework/classes4.dex"
fixture_tmp="$(mktemp -d "${TMPDIR:-/tmp}/darwin-art-fixture-input-abi.XXXXXX")"
trap 'rm -rf "$fixture_tmp"' EXIT
"$dexdump_bin" "$framework_dex" | LC_ALL=C awk '
  /Class descriptor/ { selected = ($0 ~ /ViewRootImpl\$WindowInputEventReceiver/) }
  selected && /name|type|Class descriptor/ { print }
' > "$fixture_tmp/receiver-methods"
rg -Fq "name          : '<init>'" "$fixture_tmp/receiver-methods"
rg -Fq "type          : '(Landroid/view/ViewRootImpl;Landroid/view/InputChannel;Landroid/os/Looper;)V'" "$fixture_tmp/receiver-methods"
rg -Fq "name          : 'dispose'" "$fixture_tmp/receiver-methods"
rg -Fq "name          : 'onInputEvent'" "$fixture_tmp/receiver-methods"
rg -Fq "type          : '(Landroid/view/InputEvent;)V'" "$fixture_tmp/receiver-methods"
"$dexdump_bin" "$repo_dir/_build/bootclasspath/core-libart/classes.dex" | LC_ALL=C awk '
  /Class descriptor/ { selected = index($0, "Landroid/system/Os;") != 0 }
  selected && /name|type|Class descriptor/ { print }
' > "$fixture_tmp/os-methods"
rg -Fq "name          : 'socketpair'" "$fixture_tmp/os-methods"
rg -Fq "type          : '(IIILjava/io/FileDescriptor;Ljava/io/FileDescriptor;)V'" "$fixture_tmp/os-methods"
rg -Fq "name          : 'close'" "$fixture_tmp/os-methods"
rg -Fq "type          : '(Ljava/io/FileDescriptor;)V'" "$fixture_tmp/os-methods"
for fixture_method in read write; do
  rg -Fq "name          : '$fixture_method'" "$fixture_tmp/os-methods"
done
rg -Fq "type          : '(Ljava/io/FileDescriptor;[BII)I'" "$fixture_tmp/os-methods"
# Assert the name and descriptor together: another overload must not satisfy
# a signature check intended for this factory.
fixture_assert_method() {
  LC_ALL=C awk '/name[ ]*:/ { name = $3 } /type[ ]*:/ { print name "|" $3 }' "$1" |
    rg -Fxq "'$2'|'$3'"
}
fixture_assert_method "$fixture_tmp/receiver-methods" '<init>' '(Landroid/view/ViewRootImpl;Landroid/view/InputChannel;Landroid/os/Looper;)V'
fixture_assert_method "$fixture_tmp/receiver-methods" dispose '()V'
fixture_assert_method "$fixture_tmp/os-methods" socketpair '(IIILjava/io/FileDescriptor;Ljava/io/FileDescriptor;)V'
fixture_assert_method "$fixture_tmp/os-methods" close '(Ljava/io/FileDescriptor;)V'
for fixture_method in read write; do
  fixture_assert_method "$fixture_tmp/os-methods" "$fixture_method" '(Ljava/io/FileDescriptor;[BII)I'
done
"$dexdump_bin" "$repo_dir/_build/bootclasspath/framework/classes3.dex" | LC_ALL=C awk '
  /Class descriptor/ { selected = index($0, "Landroid/os/Parcel;") != 0 }
  selected && /name|type|Class descriptor/ { print }
' > "$fixture_tmp/parcel-methods"
fixture_assert_method "$fixture_tmp/parcel-methods" obtain '()Landroid/os/Parcel;'
fixture_assert_method "$fixture_tmp/parcel-methods" recycle '()V'
fixture_assert_method "$fixture_tmp/parcel-methods" setDataPosition '(I)V'
fixture_assert_method "$fixture_tmp/parcel-methods" writeInt '(I)V'
fixture_assert_method "$fixture_tmp/parcel-methods" writeString '(Ljava/lang/String;)V'
fixture_assert_method "$fixture_tmp/parcel-methods" writeStrongBinder '(Landroid/os/IBinder;)V'
fixture_assert_method "$fixture_tmp/parcel-methods" writeFileDescriptor '(Ljava/io/FileDescriptor;)V'
echo 'fixture InputChannel ABI: actual pinned boot receiver + guest Os + raw-FD Parcel method/descriptor pairs PASS'
