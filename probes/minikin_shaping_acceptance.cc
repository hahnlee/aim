#include <unicode/uclean.h>
#include <unicode/uversion.h>

#include <cmath>
#include <cstdint>
#include <cstring>
#include <iomanip>
#include <iostream>
#include <memory>
#include <numeric>
#include <stdexcept>
#include <string>
#include <string_view>
#include <utility>
#include <vector>

#include "FreeTypeMinikinFontForTest.h"
#include "minikin/Font.h"
#include "minikin/FontCollection.h"
#include "minikin/FontFamily.h"
#include "minikin/Hyphenator.h"
#include "minikin/Layout.h"
#include "minikin/MinikinPaint.h"
#include "minikin/MinikinRect.h"
#include "minikin/Range.h"
#include "minikin/U16StringPiece.h"

namespace {

constexpr uint64_t kFnvOffset = 1469598103934665603ULL;
constexpr uint64_t kFnvPrime = 1099511628211ULL;

struct Case {
    std::string_view name;
    std::vector<std::string> font_files;
    std::vector<uint16_t> text;
    minikin::Bidi bidi;
    bool require_nonempty_bounds;
    size_t expected_glyphs;
    uint32_t expected_clusters;
};

struct Result {
    uint64_t digest;
    size_t glyphs;
    uint32_t clusters;
    uint32_t runs;
    float advance;
    minikin::MinikinRect bounds;
};

[[noreturn]] void fail(std::string_view case_name, std::string_view message) {
    throw std::runtime_error(std::string(case_name) + ": " + std::string(message));
}

void require(bool condition, std::string_view case_name, std::string_view message) {
    if (!condition) {
        fail(case_name, message);
    }
}

uint64_t mixBytes(uint64_t digest, const void* bytes, size_t size) {
    const auto* data = static_cast<const uint8_t*>(bytes);
    for (size_t i = 0; i < size; ++i) {
        digest ^= data[i];
        digest *= kFnvPrime;
    }
    return digest;
}

template <typename T>
uint64_t mixValue(uint64_t digest, const T& value) {
    return mixBytes(digest, &value, sizeof(value));
}

std::shared_ptr<minikin::FontFamily> buildFamily(const std::string& font_path) {
    auto typeface = std::make_shared<minikin::FreeTypeMinikinFontForTest>(font_path);
    std::vector<std::shared_ptr<minikin::Font>> fonts;
    fonts.push_back(minikin::Font::Builder(typeface).build());
    return minikin::FontFamily::create(std::move(fonts));
}

std::shared_ptr<minikin::FontCollection> buildCollection(const Case& test_case,
                                                         const std::string& font_dir) {
    std::vector<std::shared_ptr<minikin::FontFamily>> families;
    for (const std::string& file : test_case.font_files) {
        families.push_back(buildFamily(font_dir + "/" + file));
    }
    return minikin::FontCollection::create(families);
}

Result shapeOnce(const Case& test_case, const std::shared_ptr<minikin::FontCollection>& collection) {
    const minikin::U16StringPiece text(test_case.text);
    const minikin::Range range(0, text.size());
    minikin::MinikinPaint paint(collection);
    paint.size = 16.0f;
    paint.scaleX = 1.0f;
    paint.skewX = 0.0f;

    minikin::Layout layout(text, range, test_case.bidi, paint,
                           minikin::StartHyphenEdit::NO_EDIT,
                           minikin::EndHyphenEdit::NO_EDIT, minikin::RunFlag::WHOLE_LINE);

    require(layout.nGlyphs() != 0, test_case.name, "layout produced no glyphs");
    require(layout.getFontRunCount() != 0, test_case.name, "layout produced no font runs");
    if (test_case.expected_glyphs != 0) {
        require(layout.nGlyphs() == test_case.expected_glyphs, test_case.name,
                "unexpected glyph count");
    }

    uint32_t previous_run_end = 0;
    for (uint32_t i = 0; i < layout.getFontRunCount(); ++i) {
        const uint32_t start = layout.getFontRunStart(i);
        const uint32_t end = layout.getFontRunEnd(i);
        require(start == previous_run_end, test_case.name, "font runs are not contiguous");
        require(start < end && end <= layout.nGlyphs(), test_case.name,
                "font run is outside the glyph range");
        require(layout.getFontRunFont(i).typeface() != nullptr, test_case.name,
                "font run has no typeface");
        previous_run_end = end;
    }
    require(previous_run_end == layout.nGlyphs(), test_case.name,
            "font runs do not cover every glyph");

    uint64_t digest = kFnvOffset;
    digest = mixBytes(digest, test_case.name.data(), test_case.name.size());
    for (uint16_t code_unit : test_case.text) {
        digest = mixValue(digest, code_unit);
    }
    for (size_t i = 0; i < layout.nGlyphs(); ++i) {
        const uint32_t glyph = layout.getGlyphId(i);
        const float x = layout.getX(i);
        const float y = layout.getY(i);
        require(glyph != 0, test_case.name, "font fallback produced .notdef");
        require(std::isfinite(x) && std::isfinite(y), test_case.name,
                "glyph position is not finite");
        require(layout.typeface(i) != nullptr, test_case.name, "glyph has no typeface");
        digest = mixValue(digest, glyph);
        digest = mixValue(digest, x);
        digest = mixValue(digest, y);
    }

    const std::vector<float>& layout_advances = layout.getAdvances();
    require(layout_advances.size() == text.size(), test_case.name,
            "advance vector does not match UTF-16 input");
    float summed_advance = 0.0f;
    for (float advance : layout_advances) {
        require(std::isfinite(advance) && advance >= 0.0f, test_case.name,
                "character advance is invalid");
        summed_advance += advance;
        digest = mixValue(digest, advance);
    }
    require(std::fabs(summed_advance - layout.getAdvance()) < 0.001f, test_case.name,
            "layout advance does not equal character advances");
    require(layout.getAdvance() > 0.0f, test_case.name, "total advance is not positive");

    std::vector<float> measured_advances(text.size());
    minikin::MinikinRect bounds;
    uint32_t cluster_count = 0;
    const float measured_advance = minikin::Layout::measureText(
            text, range, test_case.bidi, paint, minikin::StartHyphenEdit::NO_EDIT,
            minikin::EndHyphenEdit::NO_EDIT, measured_advances.data(), &bounds, &cluster_count,
            minikin::RunFlag::WHOLE_LINE);
    require(std::fabs(measured_advance - layout.getAdvance()) < 0.001f, test_case.name,
            "Layout and measureText advances differ");
    require(cluster_count != 0 && cluster_count <= text.size(), test_case.name,
            "cluster count is outside UTF-16 input");
    require(cluster_count == test_case.expected_clusters, test_case.name,
            "unexpected shaping cluster count");
    require(bounds.isValid(), test_case.name, "bounds are invalid");
    if (test_case.require_nonempty_bounds) {
        require(!bounds.isEmpty(), test_case.name, "drawable text has empty bounds");
    }
    for (size_t i = 0; i < measured_advances.size(); ++i) {
        require(std::fabs(measured_advances[i] - layout_advances[i]) < 0.001f, test_case.name,
                "Layout and measureText character advances differ");
    }

    digest = mixValue(digest, cluster_count);
    digest = mixValue(digest, bounds.mLeft);
    digest = mixValue(digest, bounds.mTop);
    digest = mixValue(digest, bounds.mRight);
    digest = mixValue(digest, bounds.mBottom);
    require(digest != kFnvOffset, test_case.name, "digest was not populated");

    return Result{digest, layout.nGlyphs(), cluster_count, layout.getFontRunCount(),
                  layout.getAdvance(), bounds};
}

void runCase(const Case& test_case, const std::string& font_dir) {
    const auto collection = buildCollection(test_case, font_dir);
    const Result first = shapeOnce(test_case, collection);
    const Result cached = shapeOnce(test_case, collection);
    require(first.digest == cached.digest, test_case.name,
            "cached shaping changed the glyph digest");
    require(first.glyphs == cached.glyphs && first.clusters == cached.clusters &&
                    first.runs == cached.runs && std::fabs(first.advance - cached.advance) < 0.001f,
            test_case.name, "cached shaping changed layout metrics");

    std::cout << "case=" << test_case.name << " glyphs=" << first.glyphs
              << " clusters=" << first.clusters << " runs=" << first.runs
              << " advance=" << first.advance << " bounds=" << first.bounds.mLeft << ','
              << first.bounds.mTop << ',' << first.bounds.mRight << ',' << first.bounds.mBottom
              << " digest=" << std::hex << std::setw(16) << std::setfill('0') << first.digest
              << std::dec << '\n';
}

}  // namespace

int main(int argc, char** argv) {
    if (argc != 2) {
        std::cerr << "usage: minikin-shaping-acceptance <pinned-font-directory>\n";
        return 64;
    }

    try {
        minikin::FreeTypeMinikinFontForTestFactory::init();
        const std::string font_dir(argv[1]);
        const std::vector<Case> cases = {
                {"ascii-click", {"Ascii.ttf"}, {'C', 'l', 'i', 'c', 'k'}, minikin::Bidi::LTR,
                 true, 5, 5},
                // Arabic.ttf owns the bidi controls used by Android's Minikin tests;
                // ControlCharacters.ttf supplies drawable Alef/Beh outlines.
                {"arabic-bidi", {"Arabic.ttf", "ControlCharacters.ttf"},
                 {0x061c, 0x0627, 0x0628}, minikin::Bidi::RTL, true, 0, 3},
                {"hangul", {"Hangul.ttf"}, {0xac00, 0xac01}, minikin::Bidi::LTR, true, 2, 2},
                // The pinned font's GSUB table maps `fi` to one ligature glyph. This
                // is the explicit proof that the path entered HarfBuzz shaping.
                {"harfbuzz-ligature", {"Ligature.ttf"}, {'f', 'i'}, minikin::Bidi::LTR, true, 1,
                 1},
        };

        UVersionInfo version;
        char version_string[U_MAX_VERSION_STRING_LENGTH] = {};
        u_getVersion(version);
        u_versionToString(version, version_string);
        std::cout << "icu-version=" << version_string << " data=" << U_ICUDATA_NAME << '\n';

        for (const Case& test_case : cases) {
            runCase(test_case, font_dir);
        }
        minikin::Layout::purgeCaches();
        u_cleanup();
        std::cout << "minikin-shaping-acceptance: PASS cases=" << cases.size() << '\n';
        return 0;
    } catch (const std::exception& error) {
        minikin::Layout::purgeCaches();
        u_cleanup();
        std::cerr << "minikin-shaping-acceptance: FAIL " << error.what() << '\n';
        return 1;
    }
}
