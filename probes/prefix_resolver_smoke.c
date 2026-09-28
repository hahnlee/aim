#include <aim/prefix.h>

#include <stdio.h>
#include <string.h>

static int require_result(AimPrefixResult actual,
                          AimPrefixResult expected,
                          const char* operation) {
  if (actual == expected) {
    return 1;
  }
  fprintf(stderr, "%s result=%d expected=%d\n", operation, actual, expected);
  return 0;
}

int main(void) {
  AimPrefix* prefix = aim_prefix_create();
  if (prefix == NULL) {
    return 1;
  }
  if (!require_result(aim_prefix_add_mount(
                          prefix, 1, AIM_PREFIX_IMMUTABLE, false, "/"),
                      AIM_PREFIX_OK, "add-root") ||
      !require_result(aim_prefix_add_mount(
                          prefix, 2, AIM_PREFIX_PRIVATE, true, "/data"),
                      AIM_PREFIX_OK, "add-data") ||
      !require_result(aim_prefix_add_mount(
                          prefix, 3, AIM_PREFIX_PRIVATE, true,
                          "/data/user/0/app"),
                      AIM_PREFIX_OK, "add-app") ||
      !require_result(aim_prefix_seal(prefix), AIM_PREFIX_OK,
                      "seal")) {
    aim_prefix_destroy(prefix);
    return 2;
  }

  char normalized[128];
  char relative[128];
  AimPrefixResolution resolution;
  AimPrefixResult result = aim_prefix_resolve(
      prefix, "/data/user/0/app/files", "../cache/value", &resolution,
      normalized, sizeof(normalized), relative, sizeof(relative));
  if (!require_result(result, AIM_PREFIX_OK, "resolve") ||
      resolution.mount_id != 3 || !resolution.writable ||
      resolution.requires_directory ||
      strcmp(normalized, "/data/user/0/app/cache/value") != 0 ||
      strcmp(relative, "cache/value") != 0) {
    fprintf(stderr,
            "resolution id=%u writable=%d normalized=%s relative=%s\n",
            resolution.mount_id, resolution.writable, normalized, relative);
    aim_prefix_destroy(prefix);
    return 3;
  }

  const char non_utf8_path[] = {'/', 'd', 'a', 't', 'a', '/', (char)0xff,
                                '/', '\0'};
  result = aim_prefix_resolve(
      prefix, "/", non_utf8_path, &resolution, normalized,
      sizeof(normalized), relative, sizeof(relative));
  if (!require_result(result, AIM_PREFIX_OK, "non-utf8") ||
      resolution.mount_id != 2 || !resolution.requires_directory ||
      resolution.relative_path_length != 1 ||
      (unsigned char)relative[0] != 0xff) {
    aim_prefix_destroy(prefix);
    return 4;
  }

  if (!require_result(
          aim_prefix_resolve(prefix, "/", "../../escape", &resolution,
                                    normalized, sizeof(normalized), relative,
                                    sizeof(relative)),
          AIM_PREFIX_OK, "root-clamp") ||
      strcmp(normalized, "/escape") != 0 || resolution.mount_id != 1 ||
      !require_result(
          aim_prefix_resolve(prefix, "/", "/data/value", &resolution,
                                    normalized, 2, relative, sizeof(relative)),
          AIM_PREFIX_BUFFER_TOO_SMALL, "small-buffer")) {
    aim_prefix_destroy(prefix);
    return 5;
  }

  aim_prefix_destroy(prefix);
  puts("prefix-resolver: longest-prefix=pass normalize=pass root-clamp=pass c-abi=pass");
  return 0;
}
