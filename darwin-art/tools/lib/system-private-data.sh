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
  # init.rc post-fs-data: the /data directories system_server, vold and
  # installd work in, with init's modes. vold creates each user's directories
  # below the credential/device-encrypted roots as root; its Darwin role runs
  # as this process's host user, so those roots also grant owner write.
  local entry mode
  for entry in misc:1771 misc/user:0771 misc/profiles:0771 misc/profiles/cur:0771 \
      misc/profiles/ref:0771 app-staging:0751 resource-cache:0771 local:0751 local/tmp:0771 \
      media:0750 misc_ce:0751 misc_de:0751 system_ce:0750 system_de:0750 \
      user:0711 user_de:0711 vendor_ce:0751 vendor_de:0751; do
    directory="$private/${entry%%:*}"
    mode="${entry##*:}"
    if [[ -L "$directory" || ( -e "$directory" && ! -d "$directory" ) ]] ||
       ! mkdir -p "$directory" || ! chmod "$mode" "$directory"; then
      echo "system data directory could not be prepared: $directory" >&2
      chmod 0500 "$private"
      return 1
    fi
  done
  # Keep the namespace root sealed while AtomicFile can create/rename files
  # inside /data/system. Existing service state is deliberately preserved.
  chmod 0500 "$private"
}
