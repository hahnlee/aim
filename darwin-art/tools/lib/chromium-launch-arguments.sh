# Chromium's Android command-line-file payload is a literal, space-separated
# switch list. Configure the standalone first-run and Graphite policy without
# evaluating or reconstructing the caller's arguments.
darwin_art_configure_chromium_launch_arguments() {
  [[ "$#" == 1 ]] || {
    echo "Chromium launch configuration requires one command-line argument" >&2
    return 64
  }

  local command_line="$1"
  local token backend disabled_features
  local has_no_first_run=0 has_disable_fre=0
  local has_disable_background_networking=0 has_enable_graphite=0
  local has_vulkan_backend=0
  local restore_noglob=0

  # The command line is data, not shell input. Disable pathname expansion while
  # splitting it into literal switch tokens; command substitutions in the
  # expanded value are never re-evaluated by the shell.
  case "$-" in
    *f*) ;;
    *) set -f; restore_noglob=1 ;;
  esac
  for token in $command_line; do
    case "$token" in
      --no-first-run) has_no_first_run=1 ;;
      --disable-fre) has_disable_fre=1 ;;
      --disable-background-networking) has_disable_background_networking=1 ;;
      --enable-skia-graphite) has_enable_graphite=1 ;;
      --enable-skia-graphite=false|--enable-skia-graphite=0)
        [[ "$restore_noglob" == 0 ]] || set +f
        echo "Chromium launch arguments disable required Skia Graphite" >&2
        return 64
        ;;
      --disable-gpu|--disable-gpu=*)
        [[ "$restore_noglob" == 0 ]] || set +f
        echo "Chromium launch arguments disable the GPU" >&2
        return 64
        ;;
      --disable-skia-graphite|--disable-skia-graphite=*)
        [[ "$restore_noglob" == 0 ]] || set +f
        echo "Chromium launch arguments disable required Skia Graphite" >&2
        return 64
        ;;
      --skia-graphite-dawn-backend=*)
        backend="${token#*=}"
        if [[ "$backend" == vulkan ]]; then
          has_vulkan_backend=1
        else
          [[ "$restore_noglob" == 0 ]] || set +f
          echo "Chromium launch arguments select unsupported Graphite backend: $backend" >&2
          return 64
        fi
        ;;
      --skia-graphite-dawn-backend)
        [[ "$restore_noglob" == 0 ]] || set +f
        echo "Chromium launch arguments contain an incomplete Graphite backend switch" >&2
        return 64
        ;;
      --disable-features=*)
        disabled_features="${token#*=}"
        case ",$disabled_features," in
          *,SkiaGraphite,*)
            [[ "$restore_noglob" == 0 ]] || set +f
            echo "Chromium launch arguments disable the Skia Graphite feature" >&2
            return 64
            ;;
        esac
        ;;
    esac
  done

  [[ "$has_no_first_run" == 1 ]] || command_line="${command_line:+$command_line }--no-first-run"
  [[ "$has_disable_fre" == 1 ]] || command_line="${command_line:+$command_line }--disable-fre"
  [[ "$has_disable_background_networking" == 1 ]] || \
    command_line="${command_line:+$command_line }--disable-background-networking"
  [[ "$has_enable_graphite" == 1 ]] || \
    command_line="${command_line:+$command_line }--enable-skia-graphite"
  [[ "$has_vulkan_backend" == 1 ]] || \
    command_line="${command_line:+$command_line }--skia-graphite-dawn-backend=vulkan"

  [[ "$restore_noglob" == 0 ]] || set +f
  printf '%s\n' "$command_line"
}
