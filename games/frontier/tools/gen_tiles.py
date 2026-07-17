#!/usr/bin/env python3
"""生成俯视角像素白模瓦片库（占位素材，后期 i2i 美化替换）。确定性:固定种子。
用法: python3 games/frontier/tools/gen_tiles.py"""
import os
from PIL import Image, ImageDraw

OUT = os.path.normpath(os.path.join(os.path.dirname(__file__), "..", "assets"))
os.makedirs(OUT, exist_ok=True)
S = 32

def dark(c, f=0.62):
    return (int(c[0] * f), int(c[1] * f), int(c[2] * f), 255)

def lite(c, f=1.35):
    return (min(255, int(c[0] * f)), min(255, int(c[1] * f)), min(255, int(c[2] * f)), 255)

def save(name, img):
    img.save(os.path.join(OUT, name + ".png"))

def ground(name, base, speckle=None, n=14):
    img = Image.new("RGBA", (S, S), base)
    d = ImageDraw.Draw(img)
    d.rectangle([0, 0, S - 1, S - 1], outline=dark(base))
    if speckle:
        # Deterministic scatter (pseudo-random uses linear congruential, does not depend on random)
        s = 12345
        for _ in range(n):
            s = (s * 1103515245 + 12345) & 0x7fffffff
            x = s % S
            s = (s * 1103515245 + 12345) & 0x7fffffff
            y = s % S
            d.point((x, y), fill=speckle)
    return img

def figure(name, body, n=12):
    """俯视角小人/小物:透明底 + 居中圆身 + 深描边。"""
    img = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    d.ellipse([6, 6, S - 7, S - 7], fill=body, outline=dark(body, 0.5))
    d.ellipse([11, 9, S - 12, 18], fill=lite(body))  # highlight/head
    return img

# --- Outdoor ground ---
save("regolith", ground("regolith", (107, 99, 88, 255), dark((107, 99, 88, 255), 0.8)))
img = ground("rock", (74, 71, 66, 255)); ImageDraw.Draw(img).ellipse([8, 9, 24, 23], fill=lite((74,71,66,255),1.2), outline=dark((74,71,66,255))); save("rock", img)
img = ground("ore", (90, 82, 100, 255))
dd = ImageDraw.Draw(img)
for (x, y) in [(10, 12), (18, 9), (14, 20), (21, 18)]:
    dd.rectangle([x, y, x + 3, y + 3], fill=(196, 156, 220, 255))
save("ore", img)
save("ice", ground("ice", (159, 184, 196, 255), lite((159, 184, 196, 255))))

# --- Starting structure / interior (P1+ use) ---
img = Image.new("RGBA", (S, S), (154, 160, 168, 255))
dd = ImageDraw.Draw(img); dd.rectangle([0, 0, S - 1, S - 1], outline=(60, 64, 70, 255))
dd.line([0, 16, S, 16], fill=(60, 64, 70, 255)); dd.line([16, 0, 16, S], fill=(60, 64, 70, 255))
for (x, y) in [(5, 5), (26, 5), (5, 26), (26, 26)]:
    dd.ellipse([x, y, x + 2, y + 2], fill=(90, 96, 104, 255))
save("lander", img)
save("floor", ground("floor", (138, 122, 100, 255)))
img = Image.new("RGBA", (S, S), (58, 54, 64, 255)); ImageDraw.Draw(img).rectangle([0, 0, S - 1, S - 1], outline=lite((58, 54, 64, 255))); save("wall", img)
img = ground("conduit", (138, 122, 100, 255)); ImageDraw.Draw(img).line([0, 16, S, 16], fill=(194, 162, 74, 255), width=3); save("conduit", img)
# quarters: a bed on the floor (companion's dwelling, satisfies comfort need)
img = ground("quarters", (138, 122, 100, 255))
dd = ImageDraw.Draw(img)
dd.rectangle([7, 9, 25, 24], fill=(120, 96, 132, 255), outline=dark((120, 96, 132, 255)))  # bed
dd.rectangle([9, 11, 23, 15], fill=(208, 196, 220, 255))  # pillow/quilt
save("quarters", img)

# --- Planting/growing ---
img = ground("plot", (90, 70, 50, 255))
dd = ImageDraw.Draw(img)
for y in (10, 16, 22):
    dd.line([4, y, S - 4, y], fill=dark((90, 70, 50, 255), 0.8))
save("plot", img)
img = ground("crop", (90, 70, 50, 255))
dd = ImageDraw.Draw(img)
for (x, y) in [(9, 12), (16, 10), (23, 14), (13, 20), (20, 22)]:
    dd.ellipse([x, y, x + 4, y + 4], fill=(127, 170, 90, 255))
save("crop", img)
img = Image.new("RGBA", (S, S), (74, 122, 160, 255)); dd = ImageDraw.Draw(img)
dd.rectangle([0, 0, S - 1, S - 1], outline=dark((74, 122, 160, 255))); dd.ellipse([6, 5, S - 7, 16], fill=lite((74, 122, 160, 255)))
save("tank", img)

# --- Characters ---
save("player", figure("player", (255, 210, 122, 255)))
save("companion", figure("companion", (230, 138, 106, 255)))

print("tiles:", sorted(os.listdir(OUT)))
