# System process storage provisioning. Source from the profile launcher.
# The selected profile supplies this private /data backing root; apps receive
# their own roots and never the system root. No guest path bypass is installed.
prepare_system_private_data() {
  local private="$1" directory
  case "$private" in
    /*/private-data) ;;
    *) echo "invalid system private-data root" >&2; return 1 ;;
  esac
  for directory in "$private" "$private/system" "$private/user" "$private/user/0" "$private/user/0/android"; do
    if [[ -L "$directory" || ( -e "$directory" && ! -d "$directory" ) ]]; then
      echo "system data directory is not a real directory: $directory" >&2
      return 1
    fi
  done
  mkdir -p "$private" || return 1
  chmod 0700 "$private" || return 1
  if ! mkdir -p "$private/system" "$private/user/0/android" ||
     ! chmod 0700 "$private/system" "$private/user/0/android"; then
    chmod 0500 "$private"
    return 1
  fi
  # Keep the namespace root sealed while AtomicFile can create/rename files
  # inside /data/system. Existing service state is deliberately preserved.
  chmod 0500 "$private"
}
