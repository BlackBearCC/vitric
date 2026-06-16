# frontier 《星火》(暂定名) — 白模原型

科幻行星拓荒 × 模拟经营 × 会呼吸的活居民。在一颗荒星的一片区域,从孤身落脚建成有温度的定居点;
来投奔的伙伴由大模型驱动,记得你、有作息、会因为你怎么待这片地和他们而留下或离开。

设计稿:[`docs/superpowers/specs/2026-06-17-frontier-sim-design.md`](../../docs/superpowers/specs/2026-06-17-frontier-sim-design.md)

![whitebox](whitebox.gif)

*↑ 白模一览:俯视角荒星地表,点格子建造(floor/conduit/plot/wall),活伙伴 Pip 在家附近自己溜达。像素是占位素材,后期走"截图→图生图"美化。*

## 现在能跑什么(白模)

俯视角像素白模,目前落地了四块地基(均已无头自测):

- **P0 地基**:16×12 荒星地表区域、像素瓦片库、玩家四向移动、跟随相机。
- **P1 建设(主玩法)**:数字键 `1`–`4` 选结构(floor/wall/plot/conduit),左键在格子上建、右键拆。
- **P2 生存(压力)**:殖民地氧/电/食随时间消耗,产出结构续命(plot→氧+食、conduit→电);
  暖式求生,夹 0 不死、可恢复。
- **P4 活伙伴(魂)**:走近按 `t` 跟伙伴 Pip 说话 → 拼人设提示词调大模型 → 回复让它变心情、冒话泡。
  回复走录制通道,对话可逐位回放。

## 怎么跑

```bash
BIN=./target/release/vitric           # 改了 prelude/引擎要先 cargo build --release -p vitric-cli
$BIN check games/frontier             # 校验
$BIN run games/frontier --port 6173   # 无头 + 控制面(配合截图/注入做自测)
```

**操作**:方向键移动 · `1`–`4` 选结构 · 左键建 · 右键拆 · `t` 跟伙伴说话。

**让伙伴真开口**:需要配大模型端点 `VITRIC_LLM_URL/KEY/MODEL`。没有真模型时,用自带假端点测链路:

```bash
python3 games/frontier/tools/fake_llm.py 6190 &
VITRIC_LLM_URL=http://127.0.0.1:6190/v1/chat/completions VITRIC_LLM_KEY=x VITRIC_LLM_MODEL=stub \
  $BIN run games/frontier --port 6173
```

## 架构(落在 Vitric 上)

- 地表/结构/玩家/伙伴都是实体 + 组件(JSON,可快照/回放)。
- 建造、移动、生存速率结算走规则(when-X-then-Y)+ 脚本系统;跨实体聚合用"统计系统发事件→规则落字段→消耗系统每帧结算"。
- 伙伴的脑走 `ctx.ask`(引擎统一的对外问话 + 录制),回复经内置 `__onReply` 分发到脚本回调。
- 全程确定性:这趟经营、这段对话都能逐位存档/回放。

## 工具(`tools/`)

- `gen_tiles.py` — 生成像素白模瓦片库(确定性,固定种子)。
- `gen_scene.py` — 生成 `scenes/main.json`(地表 + 登陆舱 + 玩家 + 伙伴 + 殖民地状态)。
- `fake_llm.py` — 开发用假大模型端点,测伙伴对话链路。

## 已知白模欠账

- 美术是占位像素;最终走"关卡截图 → 图生图"美化,不在白模期锁死。
- 话泡文字会溢出、且默认点阵字体没有中文字形——要配 TTF + 文字换行(后续 UI 关)。
- 心脏 C(伙伴与经营互喂)、三层需求 + 温和离开、多伙伴、科技树/贸易、五幕曲线 — 后续阶段。
