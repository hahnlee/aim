#include <hb-ot.h>
#include <hb.h>

#include <cstdio>

int main() {
  hb_buffer_t* buffer = hb_buffer_create();
  if (buffer == nullptr || !hb_buffer_allocation_successful(buffer)) {
    return 10;
  }
  hb_buffer_add_utf8(buffer, "Click", -1, 0, -1);
  hb_buffer_guess_segment_properties(buffer);

  hb_font_t* font = hb_font_create(hb_face_get_empty());
  hb_shape(font, buffer, nullptr, 0);

  unsigned int count = 0;
  const hb_glyph_info_t* glyphs = hb_buffer_get_glyph_infos(buffer, &count);
  const hb_script_t latin = hb_unicode_script(hb_unicode_funcs_get_default(), 'A');
  const bool ok = glyphs != nullptr && count == 5 && latin == HB_SCRIPT_LATIN;
  std::printf("harfbuzz-smoke: glyphs=%u latin=%08x\n", count,
              static_cast<unsigned int>(latin));

  hb_font_destroy(font);
  hb_buffer_destroy(buffer);
  return ok ? 0 : 11;
}
