// Crafting module — data-driven recipe system with material consumption.
//
// Three components:
//   Crafting  — on the crafter entity: list of known recipe entity names
//     known — recipe ids (entity names) the entity can craft (e.g. ["sword_recipe"])
//
//   RecipeDef — on each recipe entity: static output definition
//     output       — item id produced (e.g. "sword")
//     output_count — quantity produced (e.g. 1)
//
//   RecipeInputs — on each recipe entity: parallel lists of required materials
//     items  — material item ids (e.g. ["iron", "wood"])
//     counts — quantity per material (e.g. [3, 1])
//
// Recipes are ENTITIES (data-driven, same pattern as quest entities carrying
// QuestDef + QuestObjective + QuestState). The game authors recipe entities in
// the scene; the crafter's Crafting.known lists which recipe entities it knows.
//
// Events the module handles (emitted by the game's rules):
//   craft { who, recipe } — request to craft `recipe` (entity name) from `who`
//
// Events the module emits:
//   crafted        { who, recipe, output, output_count } — craft succeeded
//   craft-rejected { who, recipe, reason }               — craft failed
//     reason ∈ {"unknown", "missing_materials"}
//
// Hard dependency: inventory module. The crafter must have an Inventory
// component — crafting reads Inventory to validate materials, then atomically
// consumes materials and adds the output. This is the same hard-dep pattern as
// the equipment module (which also directly reads/writes Inventory).
//
// Composition: crafting + inventory + loot + equipment forms the full crafting
// loop: kill enemy → loot materials → craft sword → equip sword → stronger.
// Without crafting, the only way to get items is shop-buying or looting. With
// crafting, players can gather materials and create items — a major gameplay
// loop in games like Skyrim, Minecraft, Stardew Valley.

vitric.fn("__crafting_craft", (args, ctx) => {
  const who = args.who;
  const recipe = args.recipe;
  if (!who) throw new Error("__crafting_craft: missing who");
  if (!recipe) throw new Error("__crafting_craft: missing recipe");

  // 1. Check the crafter knows this recipe.
  const known = (ctx.getField(who, "Crafting.known") || []).slice();
  if (!known.includes(recipe)) {
    ctx.emit("craft-rejected", { who, recipe, reason: "unknown" });
    return;
  }

  // 2. Read recipe definition (from the recipe entity).
  const output = String(ctx.getField(recipe, "RecipeDef.output") || "");
  const outputCount = Math.max(1, Number(ctx.getField(recipe, "RecipeDef.output_count")) || 1);
  if (!output) throw new Error("__crafting_craft: recipe " + recipe + " has no RecipeDef.output");

  const inputItems = (ctx.getField(recipe, "RecipeInputs.items") || []).slice();
  const inputCounts = (ctx.getField(recipe, "RecipeInputs.counts") || []).slice();

  // 3. Read crafter's inventory (hard-dep on inventory module).
  const invItems = (ctx.getField(who, "Inventory.items") || []).slice();
  const invCounts = (ctx.getField(who, "Inventory.counts") || []).slice();

  // 4. Validate: check all materials are present in sufficient quantity.
  // Build a map of required quantities per item id.
  const required = {};
  for (let i = 0; i < inputItems.length; i++) {
    const item = inputItems[i];
    const count = Number(inputCounts[i]) || 1;
    required[item] = (required[item] || 0) + count;
  }

  // Check inventory has enough of each required item.
  const haveMap = {};
  for (let i = 0; i < invItems.length; i++) {
    haveMap[invItems[i]] = (haveMap[invItems[i]] || 0) + (Number(invCounts[i]) || 0);
  }

  for (const item in required) {
    if ((haveMap[item] || 0) < required[item]) {
      ctx.emit("craft-rejected", { who, recipe, reason: "missing_materials" });
      return;
    }
  }

  // 5. Consume materials from inventory (mutate invCounts in place).
  for (let i = 0; i < invItems.length; i++) {
    const item = invItems[i];
    if (required[item] > 0) {
      invCounts[i] = (Number(invCounts[i]) || 0) - required[item];
      required[item] = 0; // mark as consumed
    }
  }

  // 6. Add output to inventory — either stack with existing or append.
  const outputIdx = invItems.indexOf(output);
  if (outputIdx >= 0) {
    invCounts[outputIdx] = (Number(invCounts[outputIdx]) || 0) + outputCount;
  } else {
    invItems.push(output);
    invCounts.push(outputCount);
  }

  // 7. Write back inventory.
  ctx.setField(who, "Inventory.items", invItems);
  ctx.setField(who, "Inventory.counts", invCounts);

  // 8. Emit success.
  ctx.emit("crafted", { who, recipe, output, output_count: outputCount });
});
