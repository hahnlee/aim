#!/bin/bash
# Packaging mechanism only: ELF guest bytes are not macOS executable images.
# Mach-O files may use .so names; select by representation, not extension.
darwin_art_sign_runtime_images() (
  set -o pipefail
  local runtime_directory="$1"
  [[ -d "$runtime_directory" && ! -L "$runtime_directory" && "$runtime_directory" != / ]] || {
    echo "invalid runtime image directory: $runtime_directory" >&2
    return 64
  }
  local library description
  find "$runtime_directory" -type f \( -name '*.dylib' -o -name '*.so' -o -name '*.so.*' \) -print0 |
  while IFS= read -r -d '' library; do
    description="$(/usr/bin/file -b "$library")" || return
    case "$description" in
      *Mach-O*)
        /usr/bin/codesign --force --sign - --timestamp=none "$library" >/dev/null || return
        ;;
      ELF\ *)
        # Android loader owns ELF admission/relocation. Preserve its bytes.
        ;;
      *)
        echo "unrecognized runtime library representation: $library ($description)" >&2
        return 65
        ;;
    esac
  done
)
