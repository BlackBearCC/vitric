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
# @colony:殖民地资源库存与产出速率(生存系统维护)
entities.append({
    "name": "colony",
    "components": {
        "Colony": {"oxygen": 60, "power": 60, "food": 60, "o2_rate": 0, "pow_rate": 0, "food_rate": 0, "pop": 0,
                   "o2_i": 60, "pow_i": 60, "food_i": 60},
        "Census": {"count": 1, "is_hub": 1},
        "Spawn": {"timer": 8, "cap": 4},
    },
})
# @hud_res / @hud_comp:屏幕顶部常驻 HUD(screen=true 锚定屏幕,不随相机)
entities.append({
    "name": "hud_res",
    "components": {
        "Position": {"x": 0, "y": 11.5},
        "Text": {"content": "", "size": 0.7, "color": "#cfe6ff", "screen": True},
    },
})
entities.append({
    "name": "hud_comp",
    "components": {
        "Position": {"x": 0, "y": 10.3},
        "Text": {"content": "", "size": 0.7, "color": "#ffd9a0", "screen": True},
    },
})
# @companion:第一个活伙伴(LLM 驱动,走近按 t 说话,平时自己在家附近溜达)。
# 人设现给死一个,后续改成现生成。话泡(Text)挂在伙伴身上,跟着它走。
entities.append({
    "name": "companion",
    "components": {
        "Companion": {},
        "Persona": {"name": "Pip", "archetype": "话痨技工", "traits": "热心,藏不住话,爱倒腾机器",
                    "speech": "语速快、爱用'诶''呐'、喜欢顺嘴吐槽"},
        "Mood": {"value": "好奇"},
        "ThinkReq": {"pending": 0},
        "Need": {"comfort": 50, "quarters": 0, "leave_timer": 0, "voiced": 0, "comfort_i": 50},
        "Census": {"count": 0, "is_hub": 0},
        "Wander": {"home_x": 6, "home_y": 7, "tx": 6, "ty": 7, "timer": 2},
        "Position": {"x": 6, "y": 7},
        "Velocity": {"x": 0, "y": 0},
        "Sprite": {"w": 0.9, "h": 0.9, "image": "companion.png"},
        "Text": {"content": "", "size": 0.7, "color": "#ffe9b0"},
    },
})

out = os.path.normpath(os.path.join(os.path.dirname(__file__), "..", "scenes", "main.json"))
with open(out, "w") as f:
    json.dump({"entities": entities}, f, indent=1)
print("wrote", out, "| entities:", len(entities))
