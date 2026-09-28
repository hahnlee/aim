#include <stdint.h>
#include <stdio.h>

#include <ft2build.h>
#include FT_FREETYPE_H

static uint64_t fnv1a_update(uint64_t hash, uint8_t byte) {
  return (hash ^ byte) * UINT64_C(1099511628211);
}

static uint64_t hash_u32(uint64_t hash, uint32_t value) {
  for (unsigned shift = 0; shift < 32; shift += 8) {
    hash = fnv1a_update(hash, (uint8_t)(value >> shift));
  }
  return hash;
}

int main(int argc, char** argv) {
  if (argc != 2) {
    fprintf(stderr, "usage: freetype-glyph-smoke FONT\n");
    return 2;
  }

  FT_Library library = NULL;
  FT_Face face = NULL;
  if (FT_Init_FreeType(&library) != 0 ||
      FT_New_Face(library, argv[1], 0, &face) != 0 ||
      FT_Set_Pixel_Sizes(face, 0, 32) != 0 ||
      FT_Load_Char(face, 'C', FT_LOAD_RENDER) != 0) {
    fprintf(stderr, "freetype-glyph-smoke: FreeType operation failed\n");
    if (face != NULL) {
      FT_Done_Face(face);
    }
    if (library != NULL) {
      FT_Done_FreeType(library);
    }
    return 3;
  }

  const FT_Bitmap* bitmap = &face->glyph->bitmap;
  if (bitmap->pixel_mode != FT_PIXEL_MODE_GRAY || bitmap->pitch <= 0) {
    fprintf(stderr, "freetype-glyph-smoke: unexpected bitmap layout\n");
    FT_Done_Face(face);
    FT_Done_FreeType(library);
    return 4;
  }
  uint64_t hash = UINT64_C(14695981039346656037);
  hash = hash_u32(hash, bitmap->width);
  hash = hash_u32(hash, bitmap->rows);
  hash = hash_u32(hash, (uint32_t)face->glyph->bitmap_left);
  hash = hash_u32(hash, (uint32_t)face->glyph->bitmap_top);
  hash = hash_u32(hash, (uint32_t)face->glyph->advance.x);
  for (unsigned row = 0; row < bitmap->rows; ++row) {
    const unsigned char* pixels = bitmap->buffer + row * bitmap->pitch;
    for (unsigned column = 0; column < bitmap->width; ++column) {
      hash = fnv1a_update(hash, pixels[column]);
    }
  }

  printf("FreeType Android glyph: C %ux%u advance=%ld hash=%016llx\n",
         bitmap->width, bitmap->rows, face->glyph->advance.x,
         (unsigned long long)hash);

  FT_Done_Face(face);
  FT_Done_FreeType(library);
  return 0;
}
