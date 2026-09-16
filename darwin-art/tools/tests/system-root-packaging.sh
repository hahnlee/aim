#!/bin/bash
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
source "$root/tools/lib/package-system-root.sh"
fonts="$root/_build/android16-system-fonts"
framework="$root/_prebuilt/android-16/resources/framework-res.apk"
stage="$(mktemp -d "${TMPDIR:-/tmp}/system-root-packaging.XXXXXX")"
cleanup() {
  find "$stage" -type d -exec chmod u+w {} +
  rm -rf -- "$stage"
}
trap cleanup EXIT
input="$(bash "$root/tools/prepare-runtime-system-root.sh")"
bundle="$stage/Host.app"
mkdir -p "$bundle/Contents/MacOS"
cp "$root/apps/DarwinARTManager/HostInfo.plist" "$bundle/Contents/Info.plist"
cp -c "$root/target/release/darwin-art-host" "$bundle/Contents/MacOS/darwin-art-host"
destination="$bundle/Contents/Resources/DarwinART/android/system-root.tar"
source_mode="$(stat -f %Lp "$input/system/lib64/libc++.so")"
darwin_art_package_system_root "$input" "$destination" "$fonts" "$framework"
expanded="$stage/expanded"
mkdir "$expanded"
/usr/bin/tar -xf "$destination" -C "$expanded"
cmp "$input/linkerconfig/ld.config.txt" "$expanded/linkerconfig/ld.config.txt"
cmp "$input/linkerconfig/apex.libraries.config.txt" \
  "$expanded/linkerconfig/apex.libraries.config.txt"
cmp "$input/system/lib64/libc++.so" "$expanded/system/lib64/libc++.so"
cmp "$fonts/system/etc/fonts.xml" "$expanded/system/etc/fonts.xml"
cmp "$fonts/system/etc/font_fallback.xml" "$expanded/system/etc/font_fallback.xml"
diff -qr "$fonts/system/fonts" "$expanded/system/fonts"
cmp "$framework" "$expanded/system/framework/framework-res.apk"
cmp "$root/_build/android16-system-services/services.jar" "$expanded/system/framework/services.jar"
python3 - "$root" "$destination" "$expanded" <<'PY'
import hashlib
from pathlib import Path
import sys
import tarfile
root, archive, expanded = map(Path, sys.argv[1:])
sys.path.insert(0, str(root / "tools/bootclasspath"))
from resolve import resolve
def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()
with tarfile.open(archive) as packaged:
    members = packaged.getmembers()
    for backing, logical in zip(resolve(root), resolve(root, locations=True), strict=True):
        relative = logical.removeprefix("/")
        selected = [member for member in members if member.name == relative]
        assert len(selected) == 1 and selected[0].isfile(), (logical, len(selected))
        assert digest(Path(backing)) == digest(expanded / relative), logical
print("system root: each selected boot JAR has one byte-identical Android path PASS")
PY
[[ "$(readlink "$input/apex/com.android.i18n/lib64/libbase.so")" == \
   "$(readlink "$expanded/apex/com.android.i18n/lib64/libbase.so")" ]]
[[ "$(stat -f %Lp "$input/system/lib64/libc++.so")" == "$source_mode" ]]
[[ -z "$(find "$destination" -type f -perm +222 -print -quit)" ]]
[[ ! -e "$expanded/data" && ! -e "$expanded/storage" ]]
if darwin_art_package_system_root "$input" "$destination" "$fonts" "$framework"; then
  echo 'existing destination unexpectedly overwritten' >&2
  exit 1
fi
codesign --force --sign - --timestamp=none "$bundle"
codesign --verify --deep --strict "$bundle"
echo 'system-root packaging: signed bundle archive, original ELF and symlinks preserved, overwrite rejected PASS'
