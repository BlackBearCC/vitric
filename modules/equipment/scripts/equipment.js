// Equipment module — wearable item slots with auto-unequip and stat bonus events.
//
// One component:
//   Equipment — parallel lists encoding equipped slots:
//     slots — slot name list (static, e.g. ["weapon", "armor", "accessory"])
//     items — equipped item id per slot ("" = empty, e.g. ["sword", "", "ring"])
//
// Events the module handles (emitted by the game's rules):
//   equip   { who, item, slot } — equip item from inventory to slot
//   unequip { who, slot }       — unequip item from slot back to inventory
//
// Events the module emits:
//   equipped   { who, item, slot } — item equipped (game applies stat bonus)
//   unequipped { who, item, slot } — item unequipped (game removes stat bonus)
//   equip-item-not-found { who, item } — item not in inventory
//   equip-slot-unknown   { who, slot } — slot not in Equipment.slots
//
// Composition: equipment reads/writes Inventory directly (same atomic pattern as shop).
// The module does NOT know about Health/Attack/Level — it just moves items between
// inventory and slots. The game's rules listen to `equipped`/`unequipped` and apply
// stat bonuses (e.g. +ATK for weapon, +HP for armor). This keeps the module generic.
//
// Auto-unequip: if you equip an item to an occupied slot, the old item is returned
// to inventory automatically before the new item is equipped. The game hears both
// `unequipped` (old) and `equipped` (new) events to update stats accordingly.

// Helper: find the index of an item in a parallel-lists inventory, return {idx, count}.
function invFind(items, counts, item) {
  for (let i = 0; i < items.length; i++) {
    if (items[i] === item) {
      return { idx: i, count: counts[i] || 0 };
    }
  }
  return { idx: -1, count: 0 };
}

// Helper: remove count of an item from a parallel-lists inventory (mutates in place).
function invRemove(items, counts, item, count) {
  const found = invFind(items, counts, item);
  if (found.idx < 0 || found.count < count) return false;
  counts[found.idx] -= count;
  if (counts[found.idx] <= 0) {
    items.splice(found.idx, 1);
    counts.splice(found.idx, 1);
  }
  return true;
}

// Helper: add count of an item to a parallel-lists inventory (mutates in place).
function invAdd(items, counts, item, count, capacity) {
  for (let i = 0; i < items.length; i++) {
    if (items[i] === item) {
      counts[i] += count;
      return true;
    }
  }
  if (items.length >= capacity) return false;
  items.push(item);
  counts.push(count);
  return true;
}

vitric.fn("__equip", (args, ctx) => {
  const who = args.who;
  const item = String(args.item);
  const slot = String(args.slot);
  if (!who) throw new Error("__equip: missing who");
  if (!item) throw new Error("__equip: missing item");
  if (!slot) throw new Error("__equip: missing slot");

  // Find the slot in Equipment.
  const eqSlots = ctx.getField(who, "Equipment.slots") || [];
  const eqItems = ((ctx.getField(who, "Equipment.items") || [])).slice();
  const slotIdx = eqSlots.indexOf(slot);
  if (slotIdx < 0) {
    ctx.emit("equip-slot-unknown", { who, slot });
    return;
  }

  // Check if the item is in inventory.
  const invItems = ((ctx.getField(who, "Inventory.items") || [])).slice();
  const invCounts = ((ctx.getField(who, "Inventory.counts") || [])).slice();
  const cap = ctx.getField(who, "Inventory.capacity") || 16;
  if (!invRemove(invItems, invCounts, item, 1)) {
    ctx.emit("equip-item-not-found", { who, item });
    return;
  }

  // If slot is occupied, auto-unequip the old item back to inventory.
  const oldItem = eqItems[slotIdx] || "";
  if (oldItem) {
    invAdd(invItems, invCounts, oldItem, 1, cap);
    // Emit unequipped for the old item — game removes its stat bonus.
    ctx.emit("unequipped", { who, item: oldItem, slot });
  }

  // Equip the new item.
  eqItems[slotIdx] = item;
  ctx.setField(who, "Equipment.items", eqItems);
  ctx.setField(who, "Inventory.items", invItems);
  ctx.setField(who, "Inventory.counts", invCounts);

  // Emit equipped — game applies stat bonus.
  ctx.emit("equipped", { who, item, slot });
});

vitric.fn("__unequip", (args, ctx) => {
  const who = args.who;
  const slot = String(args.slot);
  if (!who) throw new Error("__unequip: missing who");
  if (!slot) throw new Error("__unequip: missing slot");

  // Find the slot.
  const eqSlots = ctx.getField(who, "Equipment.slots") || [];
  const eqItems = ((ctx.getField(who, "Equipment.items") || [])).slice();
  const slotIdx = eqSlots.indexOf(slot);
  if (slotIdx < 0) {
    ctx.emit("equip-slot-unknown", { who, slot });
    return;
  }

  const item = eqItems[slotIdx] || "";
  if (!item) return; // slot already empty, no-op

  // Add item back to inventory.
  const invItems = ((ctx.getField(who, "Inventory.items") || [])).slice();
  const invCounts = ((ctx.getField(who, "Inventory.counts") || [])).slice();
  const cap = ctx.getField(who, "Inventory.capacity") || 16;
  invAdd(invItems, invCounts, item, 1, cap);

  // Clear slot and write back.
  eqItems[slotIdx] = "";
  ctx.setField(who, "Equipment.items", eqItems);
  ctx.setField(who, "Inventory.items", invItems);
  ctx.setField(who, "Inventory.counts", invCounts);

  ctx.emit("unequipped", { who, item, slot });
});
