# 余烬灯塔 ember — GDD 与全队合同（导演产出，改此文件=改合同）

## 一句话
黑暗高塔里向上爬,点燃 4 座火盆驱散黑暗,全亮后塔顶之门开启。点灯即玩法,光照即关卡。

## 机制
- 平台跳跃(Body/Solid);碰火盆即点燃(一次性):火盆 Light 从 0.15 拉到 1.6,发 brazier-lit
- 4 座全亮 → 发 all-lit → 塔顶 door 消失(开门) + 胜利文案;碰 door 实体前它是 Solid
- 碰 Hazard(尖刺) → 发 hero-died → 传回 spawn 点(world/set 由规则完成),死亡计数
- Ambient 暗(#262b40) + shadows:true;平台会真实挡光

## 事件表(全队接口)
brazier-lit{n} / all-lit{} / hero-died{deaths} / game-won{}

## 实体尺寸表(灰盒合同,美术贴图必须匹配,改尺寸找导演)
hero: Collider 1.3x2.1, Sprite 1.7x2.2 | brazier: Collider 1.2x1.6, Sprite 1.4x1.8
spike: Collider 1.6x0.9, Sprite 1.8x1.0 | door: Collider 2.4x3.4, Sprite 2.6x3.6
tile: 2x2 | 塔宽 x∈[-8,8],高 y∈[0,40],camera view_h 20 follow hero lerp 0.12

## 地盘
美术=assets/+animations.json+palette.json | 关卡+文案=scenes/ | 玩法+音频=rules/+scripts/+sounds/
schema/vitric.json/本文件=导演
