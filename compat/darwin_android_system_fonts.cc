#include "darwin_android_system_fonts.h"

#include <dirent.h>
#include <sys/stat.h>

#include <algorithm>
#include <cctype>
#include <cstdlib>
#include <cstring>
#include <string>
#include <vector>

struct ASystemFontIterator {
  std::vector<std::string> paths;
  size_t index = 0;
};

struct AFont {
  std::string path;
  uint16_t weight = 400;
  bool italic = false;
};

namespace {

bool IsFontFile(const std::string& path) {
  const size_t dot = path.find_last_of('.');
  if (dot == std::string::npos) return false;
  std::string extension = path.substr(dot);
  std::transform(extension.begin(), extension.end(), extension.begin(),
                 [](unsigned char value) { return std::tolower(value); });
  return extension == ".ttf" || extension == ".ttc" || extension == ".otf";
}

void AppendFonts(const std::string& directory,
                 int depth,
                 std::vector<std::string>* paths) {
  if (depth > 8) return;
  DIR* stream = opendir(directory.c_str());
  if (stream == nullptr) return;
  while (dirent* entry = readdir(stream)) {
    if (std::strcmp(entry->d_name, ".") == 0 ||
        std::strcmp(entry->d_name, "..") == 0) {
      continue;
    }
    const std::string path = directory + "/" + entry->d_name;
    struct stat status {};
    if (lstat(path.c_str(), &status) != 0) continue;
    if (S_ISDIR(status.st_mode)) {
      AppendFonts(path, depth + 1, paths);
    } else if (S_ISREG(status.st_mode) && IsFontFile(path)) {
      paths->push_back(path);
    }
  }
  closedir(stream);
}

std::string LowerBasename(const std::string& path) {
  const size_t slash = path.find_last_of('/');
  std::string result = path.substr(slash == std::string::npos ? 0 : slash + 1);
  std::transform(result.begin(), result.end(), result.begin(),
                 [](unsigned char value) { return std::tolower(value); });
  return result;
}

uint16_t FontWeight(const std::string& basename) {
  if (basename.find("thin") != std::string::npos) return 100;
  if (basename.find("extralight") != std::string::npos ||
      basename.find("ultralight") != std::string::npos) return 200;
  if (basename.find("light") != std::string::npos) return 300;
  if (basename.find("medium") != std::string::npos) return 500;
  if (basename.find("semibold") != std::string::npos ||
      basename.find("demibold") != std::string::npos) return 600;
  if (basename.find("bold") != std::string::npos) return 700;
  if (basename.find("heavy") != std::string::npos ||
      basename.find("black") != std::string::npos) return 900;
  return 400;
}

}  // namespace

extern "C" ASystemFontIterator* ASystemFontIterator_open() {
  auto* iterator = new ASystemFontIterator;
  AppendFonts("/System/Library/Fonts", 0, &iterator->paths);
  AppendFonts("/Library/Fonts", 0, &iterator->paths);
  const char* home = std::getenv("HOME");
  if (home != nullptr && home[0] == '/') {
    AppendFonts(std::string(home) + "/Library/Fonts", 0, &iterator->paths);
  }
  std::sort(iterator->paths.begin(), iterator->paths.end());
  iterator->paths.erase(
      std::unique(iterator->paths.begin(), iterator->paths.end()),
      iterator->paths.end());
  return iterator;
}

extern "C" AFont* ASystemFontIterator_next(ASystemFontIterator* iterator) {
  if (iterator == nullptr || iterator->index >= iterator->paths.size()) {
    return nullptr;
  }
  auto* font = new AFont;
  font->path = iterator->paths[iterator->index++];
  const std::string basename = LowerBasename(font->path);
  font->weight = FontWeight(basename);
  font->italic = basename.find("italic") != std::string::npos ||
                 basename.find("oblique") != std::string::npos;
  return font;
}

extern "C" void ASystemFontIterator_close(ASystemFontIterator* iterator) {
  delete iterator;
}

extern "C" void AFont_close(AFont* font) { delete font; }

extern "C" const char* AFont_getFontFilePath(const AFont* font) {
  return font == nullptr ? nullptr : font->path.c_str();
}

extern "C" uint16_t AFont_getWeight(const AFont* font) {
  return font == nullptr ? 400 : font->weight;
}

extern "C" bool AFont_isItalic(const AFont* font) {
  return font != nullptr && font->italic;
}

extern "C" const char* AFont_getLocale(const AFont*) { return ""; }

extern "C" size_t AFont_getCollectionIndex(const AFont*) { return 0; }

extern "C" size_t AFont_getAxisCount(const AFont*) { return 0; }

extern "C" uint32_t AFont_getAxisTag(const AFont*, uint32_t) { return 0; }

extern "C" float AFont_getAxisValue(const AFont*, uint32_t) { return 0.0f; }

void* aim_android_system_font_symbol(const char* symbol) {
  if (symbol == nullptr) return nullptr;
#define AIM_FONT_SYMBOL(name)                 \
  if (std::strcmp(symbol, #name) == 0) {             \
    return reinterpret_cast<void*>(&name);           \
  }
  AIM_FONT_SYMBOL(ASystemFontIterator_open)
  AIM_FONT_SYMBOL(ASystemFontIterator_next)
  AIM_FONT_SYMBOL(ASystemFontIterator_close)
  AIM_FONT_SYMBOL(AFont_close)
  AIM_FONT_SYMBOL(AFont_getFontFilePath)
  AIM_FONT_SYMBOL(AFont_getWeight)
  AIM_FONT_SYMBOL(AFont_isItalic)
  AIM_FONT_SYMBOL(AFont_getLocale)
  AIM_FONT_SYMBOL(AFont_getCollectionIndex)
  AIM_FONT_SYMBOL(AFont_getAxisCount)
  AIM_FONT_SYMBOL(AFont_getAxisTag)
  AIM_FONT_SYMBOL(AFont_getAxisValue)
#undef AIM_FONT_SYMBOL
  return nullptr;
}
