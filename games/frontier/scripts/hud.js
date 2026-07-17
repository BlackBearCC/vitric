// HUD decoration — food progress bar.
// Colony.food_i is the current food amount (0..100); every frame we compute a 10-cell bar string,
// write it to Colony.food_bar, and the rule in rules/hud.json pulls this string straight into @hud_food_lbl.UiLabel.content.
// Unicode half-block characters (█ filled / ░ light) replace bare digits, so the food level is visible at a glance.
//
// Systems:
//   food-bar     Colony → Colony.food_bar (pure string, no side effects, deterministic)

const FOOD_BAR_CELLS = 10;     // total bar cells
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