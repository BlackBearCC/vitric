// Inventory module — generic slot-based inventory with stacking, overflow, and transfer.
// Consumes pickup/drop/transfer events (dispatched by rules/inventory.json); emits item-picked-up /
// item-dropped / item-transferred / inventory-full / inventory-missing events.
// Entity referencing: `who` / `from` / `to` may be an entity name ("player") or handle ("e3v0");
// ctx.getField / ctx.setField accept both forms.

vitric.fn("__inv_pickup", (args, ctx) => {
  const who = args.who;
  const item = String(args.item);
  const count = Number(args.count) | 0;
  if (!who) throw new Error("__inv_pickup: 缺少 who（实体名或句柄）");
  if (!item) throw new Error("__inv_pickup: 缺少 item（物品 id）");
  if (count <= 0) throw new Error("__inv_pickup: count 必须 > 0");

  const items = ((ctx.getField(who, "Inventory.items") || [])).slice();
  const counts = ((ctx.getField(who, "Inventory.counts") || [])).slice();
  const cap = ctx.getField(who, "Inventory.capacity") || 16;

  // Stack onto an existing slot of the same item id.
  let remaining = count;
  for (let i = 0; i < items.length; i++) {
    if (items[i] === item) {
      counts[i] += remaining;
      remaining = 0;
      break;
    }
  }
  // No existing slot — append a new one, respecting capacity.
  if (remaining > 0) {
    if (items.length >= cap) {
      ctx.emit("inventory-full", { who, item, count: remaining });
      return;
    }
    items.push(item);
    counts.push(remaining);
  }
  ctx.setField(who, "Inventory.items", items);
  ctx.setField(who, "Inventory.counts", counts);
  ctx.emit("item-picked-up", { who, item, count });
});

vitric.fn("__inv_drop", (args, ctx) => {
  const who = args.who;
  const item = String(args.item);
  const count = Number(args.count) | 0;
  if (!who) throw new Error("__inv_drop: 缺少 who");
  if (!item) throw new Error("__inv_drop: 缺少 item");
  if (count <= 0) throw new Error("__inv_drop: count 必须 > 0");

  const items = ((ctx.getField(who, "Inventory.items") || [])).slice();
  const counts = ((ctx.getField(who, "Inventory.counts") || [])).slice();

  const idx = items.indexOf(item);
  if (idx < 0 || counts[idx] < count) {
    ctx.emit("inventory-missing", { who, item, count });
    return;
  }
  counts[idx] -= count;
  if (counts[idx] <= 0) {
    // Slot emptied — remove it so capacity is freed.
    items.splice(idx, 1);
    counts.splice(idx, 1);
  }
  ctx.setField(who, "Inventory.items", items);
  ctx.setField(who, "Inventory.counts", counts);
  ctx.emit("item-dropped", { who, item, count });
});

vitric.fn("__inv_transfer", (args, ctx) => {
  const from = args.from;
  const to = args.to;
  const item = String(args.item);
  const count = Number(args.count) | 0;
  if (!from || !to) throw new Error("__inv_transfer: 缺少 from / to");
  if (!item) throw new Error("__inv_transfer: 缺少 item");
  if (count <= 0) throw new Error("__inv_transfer: count 必须 > 0");

  const fromItems = ((ctx.getField(from, "Inventory.items") || [])).slice();
  const fromCounts = ((ctx.getField(from, "Inventory.counts") || [])).slice();
  const toItems = ((ctx.getField(to, "Inventory.items") || [])).slice();
  const toCounts = ((ctx.getField(to, "Inventory.counts") || [])).slice();
  const toCap = ctx.getField(to, "Inventory.capacity") || 16;

  const idx = fromItems.indexOf(item);
  if (idx < 0 || fromCounts[idx] < count) {
    ctx.emit("inventory-missing", { who: from, item, count });
    return;
  }
  // Check destination capacity before mutating.
  let toIdx = toItems.indexOf(item);
  if (toIdx < 0 && toItems.length >= toCap) {
    ctx.emit("inventory-full", { who: to, item, count });
    return;
  }
  // Source decrement.
  fromCounts[idx] -= count;
  if (fromCounts[idx] <= 0) {
    fromItems.splice(idx, 1);
    fromCounts.splice(idx, 1);
  }
  // Destination increment.
  if (toIdx < 0) {
    toItems.push(item);
    toCounts.push(count);
    toIdx = toItems.length - 1;
  } else {
    toCounts[toIdx] += count;
  }
  ctx.setField(from, "Inventory.items", fromItems);
  ctx.setField(from, "Inventory.counts", fromCounts);
  ctx.setField(to, "Inventory.items", toItems);
  ctx.setField(to, "Inventory.counts", toCounts);
  ctx.emit("item-transferred", { from, to, item, count });
});
