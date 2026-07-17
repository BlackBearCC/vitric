#!/usr/bin/env python3
"""生成 scenes/main.json:完整场景(地图瓦片 + UI 外壳 + 游戏实体 + 野外区 + 时间 HUD)。
确定性:特征点写死。用法: python games/frontier/tools/gen_scene.py"""
import json
import os

W, H = 28, 12
LANDER = {(7, 5), (8, 5), (7, 6), (8, 6)}
ROCK = {(2, 2), (3, 2), (13, 9), (4, 10), (12, 3)}
ORE = {(2, 9), (13, 2), (11, 10)}
ICE = {(14, 6), (1, 5), (5, 9)}
WILD_ROCK = {(19, 9), (22, 2), (27, 6), (16, 4), (21, 10)}
WILD_NODES = {
    (18, 3): "ore", (24, 8): "ore",
    (20, 6): "wood", (26, 4): "wood",
    (17, 8): "fiber", (25, 2): "fiber",
}
PLAYER = (7, 7)
NODE_COLORS = {"ore": "#5b5064", "wood": "#6b5a3a", "fiber": "#7a8a4a"}
NODE_LABELS = {"ore": "矿脉", "wood": "林木", "fiber": "纤维"}
UI_LABELS = {
    "mode_build_lbl": "建造", "mode_craft_lbl": "制作", "mode_interact_lbl": "互动",
    "build_plot_lbl": "种植台", "build_conduit_lbl": "电导管", "build_extractor_lbl": "抽水机",
    "build_quarters_lbl": "住所", "build_wall_lbl": "墙", "build_beacon_lbl": "信标",
    "craft_plank_lbl": "木板", "craft_chair_lbl": "椅子", "craft_lamp_lbl": "灯",
    "inv-ore_lbl": "", "inv-wood_lbl": "", "inv-fiber_lbl": "", "inv-seed_lbl": "",
    "inv-wheat_lbl": "", "inv-plank_lbl": "", "inv-chair_lbl": "", "inv-lamp_lbl": "",
    "hud_time_lbl": "第 1 天 · 晨",
}

entities = []

# ---- Tiles ----
for gy in range(H):
    for gx in range(W):
        p = (gx, gy)
        if p in LANDER:
            img, kind = "lander.png", "lander"
        elif p in ORE:
            img, kind = "ore.png", "ore"
        elif p in ROCK or (gx >= 16 and p in WILD_ROCK):
            img, kind = "rock.png", "rock"
        elif p in ICE:
            img, kind = "ice.png", "ice"
        else:
            img, kind = "regolith.png", "regolith"
        comps = {
            "Cell": {"kind": kind},
            "Position": {"x": gx, "y": gy},
            "Sprite": {"w": 1, "h": 1, "image": img},
        }
        if p in WILD_NODES:
            nk = WILD_NODES[p]
            comps["Node"] = {"kind": nk, "left": 5, "max": 5, "cooldown": 0}
            comps["Sprite"]["color"] = NODE_COLORS[nk]
            comps["Cell"]["kind"] = nk + "-node"
            comps["Text"] = {"content": NODE_LABELS[nk], "size": 0.3, "color": "#ffffff", "screen": False}
        entities.append({"name": "t_%d_%d" % (gx, gy), "components": comps})

# ---- Game entities ----
entities.append({"name": "player", "components": {
    "Player": {}, "Position": {"x": PLAYER[0], "y": PLAYER[1]},
    "Velocity": {"x": 0, "y": 0}, "Collider": {"w": 0.8, "h": 0.8},
    "Inventory": {}, "Sprite": {"w": 0.9, "h": 0.9, "image": "", "color": "#ffd24a"},
}})
entities.append({"name": "camera", "components": {
    "Camera": {"x": PLAYER[0], "y": PLAYER[1], "scale": 80, "follow": "player", "lerp": 0.12},
}})
entities.append({"name": "ui", "components": {"UiRoot": {}}})
entities.append({"name": "uistate", "components": {"Mode": {"value": "build"}, "Build": {"kind": "floor"}}})
entities.append({"name": "cmd", "components": {"Cmd": {}}})

# @colony:Colony + Census + Clock + day mirror fields
entities.append({"name": "colony", "components": {
    "Colony": {"stage": "起步", "day": 1, "next_drifter_day": 3, "drifters_invited": 0, "drifters_spawned": 1, "monument_built": 0},
    "Census": {"count": 0, "is_hub": 1},
    "Clock": {"day": 1, "time": 0, "tod": "晨", "last_day_emit": 1},
}})
entities.append({"name": "quest", "components": {"QuestLog": {"step": 1}}})

# ---- @companion(Pip, already at home base) ----
entities.append({"name": "companion", "components": {
    "Companion": {},
    "Persona": {"name": "Pip", "archetype": "话痨技工", "traits": "热心,藏不住话,爱倒腾机器",
                "speech": "语速快、爱用'诶''呐'、喜欢顺嘴吐槽"},
    "Mood": {"value": "好奇"}, "ThinkReq": {"pending": 0},
    "Need": {"comfort": 50, "quarters": 0, "leave_timer": 0, "voiced": 0, "comfort_i": 50},
    "Census": {"count": 0, "is_hub": 0},
    "Wander": {"home_x": 6, "home_y": 7, "tx": 6, "ty": 7, "timer": 2},
    "Position": {"x": 6, "y": 7}, "Velocity": {"x": 0, "y": 0},
    "Sprite": {"w": 0.9, "h": 0.9, "image": "", "color": "#e8963a"},
    "Text": {"content": "", "size": 0.7, "color": "#ffe9b0"},
}})

# ---- @drifter(Lio, wild roaming drifter, arrival_day=1 means present from the start) ----
entities.append({"name": "drifter", "components": {
    "Drifter": {"arrival_day": 1},
    "Persona": {"name": "Lio", "archetype": "乐天厨子", "traits": "贪吃,爱张罗,记仇又健忘",
                "speech": "热络,爱用感叹号"},
    "Mood": {"value": "好奇"}, "ThinkReq": {"pending": 0},
    "Position": {"x": 23, "y": 7}, "Collider": {"w": 0.9, "h": 0.9},
    "Sprite": {"w": 0.9, "h": 0.9, "image": "", "color": "#d4a06a"},
    "Text": {"content": "", "size": 0.7, "color": "#ffe9b0"},
}})

# ---- 3 POIs in wild zone (daily-refreshing wild points of interest) ----
POI_SPECS = [
    # (name, kind, x, y, reward_table) — reward_table: {item: [lo, hi]} rolled on explore.
    ("poi_camp",  "abandoned-camp", 18, 10, {"ore": [1, 2], "wheat": [2, 4], "fiber": [1, 3]}),
    ("poi_cave",  "cave-entrance",  23, 2,  {"ore": [3, 5]}),                 # high-risk high-reward
    ("poi_wreck", "shipwreck",      26, 5,  {"wheat": [3, 5], "plank": [1, 2]}),
]
POI_LABELS = {"abandoned-camp": "废弃营地", "cave-entrance": "洞穴入口", "shipwreck": "沉船"}
POI_COLORS = {"abandoned-camp": "#8b6f47", "cave-entrance": "#5a4a6a", "shipwreck": "#4a5a6a"}
for name, kind, x, y, rewards in POI_SPECS:
    entities.append({"name": name, "components": {
        "Position": {"x": x, "y": y},
        "Sprite": {"w": 1.6, "h": 1.6, "color": POI_COLORS[kind], "image": ""},
        "Collider": {"w": 1.6, "h": 1.6},
        "Poi": {
            "kind": kind,
            "state": "fresh",
            "cooldown": 0,
            "reward_table": json.dumps(rewards),
        },
        "Text": {"content": POI_LABELS[kind], "size": 0.4, "color": "#ffe070", "screen": False},
    }})

# ---- UI shell ----
def ui_entity(name, ui, extra=None):
    comps = {"Ui": ui}
    if extra:
        comps.update(extra)
    entities.append({"name": name, "components": comps})

ui_entity("hud_bar", {"anchor": "top-center", "parent": "ui", "oy": 12, "w": 1180, "h": 48},
          {"Panel": {"color": "#161a24"}})
ui_entity("hud_res", {"anchor": "stretch", "parent": "hud_bar"},
          {"UiLabel": {"size": 26, "color": "#e8e8ee", "align": "center"}})

# Time HUD: small plate at bottom-right, shows Day N · time-of-day
ui_entity("hud_time", {"anchor": "bottom-right", "parent": "ui", "ox": -16, "oy": -16, "w": 240, "h": 44},
          {"Panel": {"color": "#161a24"}})
ui_entity("hud_time_lbl", {"anchor": "stretch", "parent": "hud_time"},
          {"UiLabel": {"content": UI_LABELS["hud_time_lbl"], "size": 26, "color": "#cfe6ff", "align": "center"}})

ui_entity("mode_box", {"anchor": "top-left", "parent": "ui", "ox": 16, "oy": 72, "w": 128, "h": 148},
          {"Container": {"kind": "VBox", "gap": 8, "pad": 6, "main": "start", "cross": "center"}})

for a in ["build", "craft", "interact"]:
    ui_entity(f"mode_{a}", {"anchor": "top-left", "parent": "mode_box", "w": 128, "h": 42},
              {"Panel": {"color": "#2c3550"}, "Button": {"action": f"mode-{a}", "state": "normal"}})
    ui_entity(f"mode_{a}_lbl", {"anchor": "stretch", "parent": f"mode_{a}"},
              {"UiLabel": {"content": UI_LABELS[f"mode_{a}_lbl"], "size": 28, "color": "#ffffff", "align": "center"}})

# Build menu: add plot2 + monument as two new options
ui_entity("build_menu", {"anchor": "top-left", "parent": "ui", "ox": 16, "oy": 236, "w": 152, "h": 360},
          {"Container": {"kind": "VBox", "gap": 6, "pad": 6, "main": "start", "cross": "center"}})
for b in ["plot", "conduit", "extractor", "quarters", "wall", "beacon", "plot2", "monument"]:
    ui_entity(f"build_{b}", {"anchor": "top-left", "parent": "build_menu", "w": 152, "h": 36},
              {"Panel": {"color": "#33405e"}, "Button": {"action": f"pick-{b}", "state": "normal"}})
    ui_entity(f"build_{b}_lbl", {"anchor": "stretch", "parent": f"build_{b}"},
              {"UiLabel": {"content": UI_LABELS[f"build_{b}_lbl"] if f"build_{b}_lbl" in UI_LABELS else b, "size": 24, "color": "#ffffff", "align": "center"}})

# Craft menu
ui_entity("craft_menu", {"anchor": "top-left", "parent": "ui", "ox": 16, "oy": 236, "w": 152, "h": 156},
          {"Container": {"kind": "VBox", "gap": 8, "pad": 6, "main": "start", "cross": "center"}})
for c in ["plank", "chair", "lamp"]:
    ui_entity(f"craft_{c}", {"anchor": "top-left", "parent": "craft_menu", "w": 152, "h": 40},
              {"Panel": {"color": "#3a3357"}, "Button": {"action": f"craft-{c}", "state": "normal"}})
    ui_entity(f"craft_{c}_lbl", {"anchor": "stretch", "parent": f"craft_{c}"},
              {"UiLabel": {"content": UI_LABELS[f"craft_{c}_lbl"], "size": 26, "color": "#ffffff", "align": "center"}})

# Inventory grid
ui_entity("inv_grid", {"anchor": "bottom-left", "parent": "ui", "ox": 16, "oy": -16, "w": 392, "h": 150},
          {"Container": {"kind": "Grid", "gap": 6, "pad": 6, "columns": 4, "main": "start", "cross": "start"},
           "Panel": {"color": "#14171f"}})
for item in ["ore", "wood", "fiber", "seed", "wheat", "plank", "chair", "lamp"]:
    ui_entity(f"inv-{item}", {"anchor": "top-left", "parent": "inv_grid"},
              {"Panel": {"color": "#242a38"}})
    ui_entity(f"inv-{item}_lbl", {"anchor": "stretch", "parent": f"inv-{item}"},
              {"UiLabel": {"size": 24, "color": "#ffd24a", "align": "center"}})

# Quest banner
ui_entity("quest_banner", {"anchor": "top-right", "parent": "ui", "ox": 16, "oy": 12, "w": 520, "h": 52},
          {"Panel": {"color": "#161a24"}})
ui_entity("quest_banner_lbl", {"anchor": "stretch", "parent": "quest_banner"},
          {"UiLabel": {"size": 26, "color": "#cfe6ff", "align": "center"}})

out = os.path.normpath(os.path.join(os.path.dirname(__file__), "..", "scenes", "main.json"))
with open(out, "w") as f:
    json.dump({"entities": entities}, f, indent=1)
print("wrote", out, "| entities:", len(entities))
