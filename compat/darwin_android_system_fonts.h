#pragma once

#include <cstddef>
#include <cstdint>

struct ASystemFontIterator;
struct AFont;

extern "C" ASystemFontIterator* ASystemFontIterator_open();
extern "C" AFont* ASystemFontIterator_next(ASystemFontIterator* iterator);
extern "C" void ASystemFontIterator_close(ASystemFontIterator* iterator);

extern "C" void AFont_close(AFont* font);
extern "C" const char* AFont_getFontFilePath(const AFont* font);
extern "C" uint16_t AFont_getWeight(const AFont* font);
extern "C" bool AFont_isItalic(const AFont* font);
extern "C" const char* AFont_getLocale(const AFont* font);
extern "C" size_t AFont_getCollectionIndex(const AFont* font);
extern "C" size_t AFont_getAxisCount(const AFont* font);
extern "C" uint32_t AFont_getAxisTag(const AFont* font, uint32_t axis_index);
extern "C" float AFont_getAxisValue(const AFont* font, uint32_t axis_index);

void* aim_android_system_font_symbol(const char* symbol);
