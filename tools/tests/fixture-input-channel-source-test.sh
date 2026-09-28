#!/bin/sh
set -eu

repo_dir=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
source_file="$repo_dir/probes/fixture_input_channel.cc"
header_file="$repo_dir/probes/fixture_input_channel.h"
ndk_root=${ANDROID_NDK_HOME:-$HOME/Library/Android/sdk/ndk/28.2.13676358}
ndk_include="$ndk_root/toolchains/llvm/prebuilt/darwin-x86_64/sysroot/usr/include"
cxx=${CXX:-c++}

[ -f "$source_file" ] && [ -f "$header_file" ]
[ -f "$ndk_include/jni.h" ] || exit 2

# Keep this fixture owner independent from private receiver registries and
# product transport symbols. The ABI test supplies runtime-side signature
# evidence; this gate checks that the fixture TU uses the same public Java
# boundary and does not regress to a local InputChannel pair.
! rg -n 'receiver_(admission|jni_resources|registry)|receiver_routing_access|runtime/framework/input|openInputChannelPair|mInputEventReceiver|->channel' \
  "$source_file" "$header_file"
rg -q '"socketpair"' "$source_file"
rg -q 'kSockStream \| kSockNonblock' "$source_file"
rg -q '"writeInt"' "$source_file"
rg -q '"writeStrongBinder"' "$source_file"
rg -q '"writeString"' "$source_file"
rg -q '"writeFileDescriptor"' "$source_file"
rg -q '"setDataPosition"' "$source_file"
rg -q '"readFromParcel"' "$source_file"
rg -q 'ViewRootImpl\$WindowInputEventReceiver' "$source_file"
rg -q 'Landroid/view/ViewRootImpl;Landroid/view/InputChannel;Landroid/os/Looper;' "$source_file"
rg -q '"dispose"' "$source_file"

"$cxx" -std=c++20 -Wall -Wextra -Werror -fsyntax-only \
  -I"$repo_dir" -I"$repo_dir/compat" -idirafter "$ndk_include" \
  "$source_file"
echo "fixture-input-channel-source: PASS"
