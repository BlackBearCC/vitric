#!/usr/bin/env python3
"""生成 P0 俯视角白模区域 scenes/main.json:一片荒星地表 + 起始登陆舱 + 玩家 + 跟随相机。
确定性:特征点写死。用法: python3 games/frontier/tools/gen_scene.py"""
import json
import os

W, H = 16, 12
LANDER = {(7, 5), (8, 5), (7, 6), (8, 6)}     # 2x2 起始登陆舱
ROCK = {(2, 2), (3, 2), (13, 9), (4, 10), (12, 3)}
ORE = {(2, 9), (13, 2), (11, 10)}
ICE = {(14, 6), (1, 5), (5, 9)}
PLAYER = (8, 7)

entities = []
for gy in range(H):
    for gx in range(W):
        p = (gx, gy)
        if p in LANDER:
            img, kind = "lander.png", "lander"
        elif p in ORE:
            img, kind = "ore.png", "ore"
        elif p in ROCK:
            img, kind = "rock.png", "rock"
        elif p in ICE:
            img, kind = "ice.png", "ice"
        else:
            img, kind = "regolith.png", "regolith"
        entities.append({
            "name": "t_%d_%d" % (gx, gy),
            "components": {
                "Cell": {"kind": kind},
                "Position": {"x": gx, "y": gy},
                "Sprite": {"w": 1, "h": 1, "image": img},
            },
        })

entities.append({
    "name": "player",
    "components": {
        "Player": {},
        "Position": {"x": PLAYER[0], "y": PLAYER[1]},
        "Velocity": {"x": 0, "y": 0},
        "Sprite": {"w": 0.9, "h": 0.9, "image": "player.png"},
    },
})
entities.append({
    "name": "camera",
    "components": {
        "Camera": {"x": PLAYER[0], "y": PLAYER[1], "scale": 18, "follow": "player", "lerp": 0.12},
    },
})
# @ui:建造选择状态(数字键改 Build.kind,点击建造时读它)
entities.append({
    "name": "ui",
    "components": {"Build": {"kind": "floor"}},
})

out = os.path.normpath(os.path.join(os.path.dirname(__file__), "..", "scenes", "main.json"))
with open(out, "w") as f:
    json.dump({"entities": entities}, f, indent=1)
print("wrote", out, "| entities:", len(entities))
