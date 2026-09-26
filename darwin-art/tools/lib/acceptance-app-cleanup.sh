# Acceptance suites end the exact app processes they started, on success and
# failure, so the next suite starts from fresh products (#12). Quit goes
# through the product path (ADR 0011: the app's Quit removes its task and the
# process exits); SIGTERM is the fallback for a process that does not exit.
darwin_art_acceptance_started_pids=()

darwin_art_acceptance_track() {
  darwin_art_acceptance_started_pids+=("$1")
}

darwin_art_acceptance_quit_started() {
  local root="$1" pid
  for pid in "${darwin_art_acceptance_started_pids[@]}"; do
    kill -0 "$pid" 2>/dev/null || continue
    swift "$root/tools/macos-app-quit.swift" "$pid" >/dev/null 2>&1 || true
  done
  for pid in "${darwin_art_acceptance_started_pids[@]}"; do
    for _ in {1..40}; do
      kill -0 "$pid" 2>/dev/null || break
      sleep 0.25
    done
    if kill -0 "$pid" 2>/dev/null; then
      echo "acceptance cleanup: pid $pid did not quit; sending SIGTERM" >&2
      kill -TERM "$pid" 2>/dev/null || true
    fi
  done
}
