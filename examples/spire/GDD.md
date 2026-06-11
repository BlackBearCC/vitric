# 灰烬尖塔 spire — GDD 与全队合同(导演产出)

## 一句话
杀戮尖塔式单场遭遇战+暗黑地牢氛围:抽牌打牌过回合,打死双敌获胜,血尽则败。键盘操作(1-5 出牌,E 结束回合)。

## 战斗循环
回合开始抽满 5 张/能量回 3 → 按 1-5 出对应手牌(够能量才生效) → E 结束回合 → 敌人按意图行动 → 循环。
敌人头顶 Text 显示意图(ATK 8 / DEF 6)。玩家 HP 70,敌人 A 斩击者 HP 28 / B 守卫 HP 40。

## 卡牌表(玩法 agent 实现,文案在此定死)
STRIKE 攻6 费1 ×5 | DEFEND 甲5 费1 ×4 | BASH 攻8+易伤 费2 ×1
CLEAVE 攻4全体 费1 ×1 | IRON WAVE 攻5甲5 费2 ×1 — 牌库 12 张,洗牌用 ctx.random
## 状态机(全在组件,快照安全)
Battle{phase:enum[player,enemy,won,lost], energy, turn} 单例;Deck{draw:list,discard:list,hand:list}(存牌名)
Unit{hp,maxhp,block,vuln} 挂玩家/敌人;敌人 Intent{kind,value}
## 事件表
card-played{name,cost} / turn-ended / enemy-acted{n} / battle-won / battle-lost / damaged{target,amount}
## 布局(尺寸合同,美术贴图必须匹配)
玩家立绘 3x4 @(-7,0) 敌A 2.6x3.4 @(4,0.2) 敌B 3x3.8 @(7.5,0)
手牌:卡面 2.2x3 一排五张 y=-6 起点 x=-5 间距 2.5;HP/能量 Text screen 锚定
火把 ×2 (±10,3) Light 暖光,Ambient #1c1822 + Bloom;背景石墙程序砖
## 地盘
美术=assets/animations/palette | 玩法=rules/scripts/sounds | 场景=scenes/(灰盒先行) | schema/本文件=导演
