// HUD 装饰元素 — 食物进度条。
// Colony.food_i 是 0..100 的当前食物量,每帧算一个 10 格的 bar 串,
// 写入 Colony.food_bar,然后 @hud_food_lbl.UiLabel.content 走规则/hud.json 直接拉这个串。
// 用 Unicode 半块字符(█ 实心 / ░ 浅色)代替裸数字,直观看出食物水位。
//
// 系统:
//   food-bar     Colony → Colony.food_bar(纯字符串,无副作用,确定性)

const FOOD_BAR_CELLS = 10;     // bar 总格数
const FOOD_BAR_FILLED = "\u2588"; // █
const FOOD_BAR_EMPTY  = "\u2591"; // ░

vitric.system("food-bar", { query: ["Colony"], writes: ["Colony"] }, (entities, ctx) => {
  for (const e of entities) {
    const food = Math.round(e.Colony.food_i || 0);
    const clamped = food < 0 ? 0 : (food > 100 ? 100 : food);
    const filled = Math.round((clamped / 100) * FOOD_BAR_CELLS);
    let bar = "";
    for (let i = 0; i < FOOD_BAR_CELLS; i++) {
      bar += i < filled ? FOOD_BAR_FILLED : FOOD_BAR_EMPTY;
    }
    e.Colony.food_bar = "食 " + clamped + "/100 [" + bar + "]";
  }
});