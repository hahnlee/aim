# Package-scoped host launch preferences. These are defaults for the existing
# Android command-line-file transport, never shell commands or runtime policy.
darwin_art_load_app_launch_arguments() {
  local config="$1"
  [[ -e "$config" || -L "$config" ]] || return 0
  [[ -f "$config" && ! -L "$config" ]] || {
    echo "app launch arguments must be a regular non-symlink file: $config" >&2
    return 65
  }
  [[ "$(wc -c < "$config")" -le 16384 ]] || {
    echo "app launch arguments file exceeds 16 KiB" >&2
    return 65
  }
  local line magic="" filename="" arguments="" saw_filename=0 saw_arguments=0
  while IFS= read -r line || [[ -n "$line" ]]; do
    if [[ -z "$magic" ]]; then
      magic="$line"
      [[ "$magic" == darwin-art-app-launch-arguments-v1 ]] || return 65
      continue
    fi
    case "$line" in
      command-line-file=*)
        [[ "$saw_filename" == 0 ]] || return 65
        saw_filename=1
        filename="${line#command-line-file=}"
        ;;
      command-line=*)
        [[ "$saw_arguments" == 0 ]] || return 65
        saw_arguments=1
        arguments="${line#command-line=}"
        ;;
      *) echo "invalid app launch arguments field" >&2; return 65 ;;
    esac
  done < "$config"
  [[ "$saw_filename" == 1 && "$saw_arguments" == 1 ]] || return 65
  case "$filename" in
    ''|.|..|*/*|*$'\r'*) return 65 ;;
  esac
  [[ "$arguments" != *$'\r'* ]] || return 65
  # A caller's explicit environment (including an empty value) overrides the
  # whole setting. Do not merge switches or duplicate Chromium feature lists.
  if [[ "${DARWIN_ART_APP_COMMAND_LINE_FILE+x}" != x ]]; then
    export DARWIN_ART_APP_COMMAND_LINE_FILE="$filename"
  fi
  if [[ "${DARWIN_ART_APP_COMMAND_LINE+x}" != x ]]; then
    export DARWIN_ART_APP_COMMAND_LINE="$arguments"
  fi
}
