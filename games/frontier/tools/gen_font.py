#!/usr/bin/env python3
"""从系统 Noto Sans CJK 提取一份单体中文字体到 fonts/cjk.otf。
引擎(ab_glyph)只吃单字体,系统里的 CJK 字体都是 .ttc 集合,所以这里抽 face 0 并子集化。
字体本身不入库(见 .gitignore),克隆后跑这个脚本生成。
用法: python3 games/frontier/tools/gen_font.py"""
import os
import subprocess
import sys

CANDIDATES = [
    "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/opentype/noto/NotoSerifCJK-Regular.ttc",
]
OUT = os.path.normpath(os.path.join(os.path.dirname(__file__), "..", "fonts", "cjk.otf"))
# Subset: ASCII + Latin Supplement + common punctuation/fullwidth + CJK unified Han characters. Covers the in-game Chinese;
# real LLM may occasionally emit rare characters outside the range (missing glyphs — acceptable for the white-model stage).
RANGES = "U+0020-007E,U+00A0-00FF,U+2000-206F,U+3000-303F,U+4E00-9FFF,U+FF00-FFEF"

src = next((p for p in CANDIDATES if os.path.exists(p)), None)
if not src:
    sys.exit("找不到系统 Noto CJK 字体,装一个或改 CANDIDATES:\n  " + "\n  ".join(CANDIDATES))

os.makedirs(os.path.dirname(OUT), exist_ok=True)
subprocess.run(
    [sys.executable, "-m", "fontTools.subset", src, "--font-number=0",
     "--unicodes=" + RANGES, "--output-file=" + OUT],
    check=True,
)
print("wrote %s (%.1f MB) from %s" % (OUT, os.path.getsize(OUT) / 1e6, src))
