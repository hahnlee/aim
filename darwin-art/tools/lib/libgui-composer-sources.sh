# Original libgui composer/listener dependency group. The caller supplies the
# verified shadow/generated roots, private stage and common flags array.
# Outputs composer_sources/composer_objects for linking and source identity.
build_libgui_composer_sources() {
  composer_sources=("$shadow/libs/gui/WindowInfosListenerReporter.cpp"
    "$shadow/libs/gui/WindowInfosUpdate.cpp" "$shadow/libs/gui/DisplayInfo.cpp")
  local unit source object
  for unit in ISurfaceComposer ISurfaceComposerClient IWindowInfosListener \
    IWindowInfosPublisher WindowInfosListenerInfo IDisplayEventConnection \
    StalledTransactionInfo HdrConversionCapability DisplayDecorationSupport; do
    composer_sources+=("$generated/gui/src/android/gui/$unit.cpp")
  done
  composer_objects=()
  for source in "${composer_sources[@]}"; do
    object="$stage/Composer-$(basename "${source%.cpp}").o"
    xcrun clang++ "${flags[@]}" -UANDROID_UTILS_REF_BASE_DISABLE_IMPLICIT_CONSTRUCTION \
      -I"$project_root/_aosp/frameworks/native/opengl/include" \
      -c "$source" -o "$object"
    composer_objects+=("$object")
  done
}
