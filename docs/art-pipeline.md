# 美术管线：一个人出整套和谐素材

AI 出图（MiniMax / Seedream 都行）最大的问题不是不好看，是张张色调不一样，拼进同一个画面就散。
Vitric 的解法：图随便生成，最后 `vitric assets` 一条命令把全项目压到同一张色板上。

## ① 出图：提示词骨架

- **同一角色多姿势一张图出**：「idle / walk / jump 三个姿势，横向排列」——分开生成就不是同一个人了
- **风格词每次原样复用**：比如 `flat colors, thick outlines, game sprite`，别每张换说法
- **纯白背景**，方便下一步抠图
- 结尾加 **`NOT photorealistic`**，挡掉写实质感

## ② 切图、白底转透明

把大图按姿势切成单张 PNG，白底转成 alpha=0。任何图片工具都行。
半透明边缘不用处理，引擎和下一步都会原样保留 alpha。

## ③ 统一色板

```bash
vitric assets <项目目录> [--colors N] [--height H]
```

收集 `assets/` 下所有 PNG 的颜色，提出一张 N 色（默认 32）的共享色板，再把每张图的颜色吸附上去。
`--height H` 会先把高于 H 的图按最近邻缩到 H（保持比例）——要像素风就加这个。

跑之前原件会自动备份到 `assets_original/`；目录已存在会拒绝执行，确认不要了再手动删。

仓库自带的 glow 示例：

```bash
./target/release/vitric assets examples/glow --colors 16
```

跑完 stdout 是 JSON 报告（图片数、色板、缩放数、前后字节数），同样的输入永远出同样的结果。

## ④ 命名与动画约定

文件名按 `角色-动作帧.png`：`hero-idle.png` `hero-walk1.png` `hero-walk2.png`，
`animations.json` 里的 clip 直接引用这些文件（动画细节见 agent-guide「动画」一节）。

## ⑤ 法线贴图：一条命令给素材加浮雕

```bash
vitric assets <项目目录> --normals            # 程序化（默认）
vitric assets <项目目录> --normals --normals-ai  # 豆包 Seedream 图生图
```

给 `assets/` 里每张没有 `_n` 配对的 PNG 生成 `xxx_n.png`。渲染侧零配置：
`hero.png` 旁边有 `hero_n.png`，光照开启时（场上有 `Ambient`）自动有浮雕感——
灯从哪边来，哪边亮（agent-guide「光照」一节有公式）。

两条生成路的取舍：

- **程序化（`--normals`）**：高度 = 亮度×alpha + 轮廓 3px 倒角，Sobel 求梯度。
  **确定性**（同输入同字节，可进 CI/断言）、离线、免费、秒出。浮雕来自亮度差和
  轮廓——像素风/形状简单的精灵效果很好；它"看不懂"画的内容（画上去的高光会被
  当成凸起）。注意：图外按透明算，整图无 alpha 也会有一圈边框倒角——无缝拼接的
  贴图块别走这条路。
- **AI（`--normals-ai`）**：模型看懂内容再出法线（鼻梁真的凸、眼窝真的凹），
  适合手绘/复杂素材。但**不确定**（重跑不同结果——生成完把 `_n` 进版本库，当素材
  不当产物）、要网络和 `ARK_API_KEY`、按张计费。AI 出的向量不守物理，引擎会逐像素
  合法化（重新归一化、z 夹 ≥0.1、剪影 alpha 回填漫反射的）后才进光照。
  模型默认 doubao-seedream-5-0-260128，可用 `VITRIC_NORMALS_MODEL` 覆盖。

两条路都只**新增**文件：已有 `_n` 配对的跳过，从不覆盖。`_n` 文件也永远不参与
③ 的色板量化（法线编码的是向量不是颜色，量化会毁掉它们）。

## ⑥ palette.json：后补素材自动入伙

第一次跑会把色板写进项目根的 `palette.json`——这就是项目的官方色板。
之后再出的新图放进 `assets/`，用锁定模式跑：

```bash
vitric assets <项目目录> --palette-lock
```

跳过提取，直接按已有色板量化新图——新素材和老素材永远一个调。
