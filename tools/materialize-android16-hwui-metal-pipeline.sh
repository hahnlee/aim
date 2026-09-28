#!/bin/bash
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
source_hwui="$root/_aosp/frameworks/base/libs/hwui"
out="${1:-$root/_build/hwui-metal-pipeline/patched-hwui}"
[[ -d "$source_hwui" ]] || { echo "hwui-metal-pipeline: missing $source_hwui" >&2; exit 2; }
mkdir -p "$out/platform/host/pipeline/skia"
cp -R "$source_hwui"/. "$out"/
cp "$root/upstream/android16-hwui-metal-pipeline/SkiaMetalPipeline.h" \
   "$out/platform/host/pipeline/skia/SkiaMetalPipeline.h"
cp "$root/upstream/android16-hwui-metal-pipeline/SkiaMetalPipeline.mm" \
   "$out/platform/host/pipeline/skia/SkiaMetalPipeline.mm"

# These edits are deliberately checked against exact Android 16 anchors.  The
# host tree remains untouched; callers compile this materialized tree together
# with the pinned HWUI sources and can retain the CPU pipeline as fallback.
python3 - "$source_hwui" "$out" <<'PY'
import pathlib, shutil, sys
src = pathlib.Path(sys.argv[1]); dst = pathlib.Path(sys.argv[2])
for name in ("Properties.h", "Properties.cpp"):
    target = dst / name
    target.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(src / name, target)
text = (dst / "Properties.h").read_text()
old = "enum class RenderPipelineType { SkiaGL, SkiaVulkan, SkiaCpu, NotInitialized = 128 };"
if text.count(old) != 1: raise SystemExit("RenderPipelineType anchor changed")
(dst / "Properties.h").write_text(text.replace(old, "enum class RenderPipelineType { SkiaGL, SkiaVulkan, SkiaCpu, SkiaMetal, NotInitialized = 128 };"))
text = (dst / "Properties.cpp").read_text()
if '#include "darwin_hwui_gpu_mode.h"' not in text:
    text = text.replace('#include "Properties.h"', '#include "Properties.h"\n#include "darwin_hwui_gpu_mode.h"', 1)
anchor = "    bool useVulkan = use_vulkan().value_or(false);"
if text.count(anchor) != 1: raise SystemExit("pipeline selection anchor changed")
text = text.replace(anchor, '#if defined(AIM_HWUI_GPU)\n    if (aim::hwui_gpu_enabled()) return RenderPipelineType::SkiaMetal;\n#endif\n' + anchor, 1)
(dst / "Properties.cpp").write_text(text)
for name in ("CanvasContext.cpp", "RenderThread.cpp"):
    shutil.copy2(src / "renderthread" / name, dst / "renderthread" / name)
text = (dst / "renderthread/CanvasContext.cpp").read_text()
text = text.replace('#include "pipeline/skia/SkiaCpuPipeline.h"', '#include "pipeline/skia/SkiaCpuPipeline.h"\n#include "platform/host/pipeline/skia/SkiaMetalPipeline.h"', 1)
anchor = "#ifndef __ANDROID__\n        case RenderPipelineType::SkiaCpu:"
if text.count(anchor) != 1: raise SystemExit("CanvasContext pipeline anchor changed")
needle = "#ifndef __ANDROID__\n        case RenderPipelineType::SkiaCpu:"
text = text.replace(needle, '        case RenderPipelineType::SkiaMetal:\n            return new CanvasContext(thread, translucent, rootRenderNode, contextFactory,\n                    std::make_unique<skiapipeline::SkiaMetalPipeline>(thread), uiThreadId, renderThreadId);\n' + needle, 1)
(dst / "renderthread/CanvasContext.cpp").write_text(text)
text = (dst / "renderthread/RenderThread.cpp").read_text()
text = text.replace('if (Properties::getRenderPipelineType() == RenderPipelineType::SkiaGL) {', 'if (Properties::getRenderPipelineType() == RenderPipelineType::SkiaGL ||\n        Properties::getRenderPipelineType() == RenderPipelineType::SkiaMetal) {', 1)
text = text.replace('case RenderPipelineType::SkiaVulkan:\n            return "Skia (Vulkan)";', 'case RenderPipelineType::SkiaVulkan:\n            return "Skia (Vulkan)";\n        case RenderPipelineType::SkiaMetal:\n            return "Skia (Metal)";', 1)
(dst / "renderthread/RenderThread.cpp").write_text(text)
PY
echo "hwui-metal-pipeline: materialized=$out pipeline=SkiaMetal fallback=SkiaCpu"
