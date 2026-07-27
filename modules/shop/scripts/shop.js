// Shop module — buy/sell items with currency, atomic inventory mutation.
//
// One component:
//   Shop — parallel lists encoding the shop catalog (same pattern as Dialogue/LootTable):
//     currency — item id used as currency (default "coin"); the game decides what counts as money
//     items    — item ids for sale (e.g. ["potion", "sword"])
//     prices   — buy price per item, in currency units (e.g. [10, 50])
//     stocks   — stock count per item; -1 = infinite, 0 = sold out, N = limited (e.g. [-1, 3])
//
// Events the module handles (emitted by the game's rules):
//   shop-buy  { who, shop, item, count } — buy count of item from shop for who
//   shop-sell { who, shop, item, count } — sell count of item from who's inventory to shop
//
// Events the module emits:
//   item-bought            { who, shop, item, count, total_price } — purchase completed
//   item-sold              { who, shop, item, count, total_price } — sale completed
//   shop-not-for-sale      { who, shop, item }                     — item not in shop catalog
//   shop-out-of-stock      { who, shop, item, available }          — not enough stock
//   shop-insufficient-funds { who, item, count, needed, have }     — buyer can't afford
//   shop-inventory-full    { who, item, count }                    — buyer's inventory full
//   shop-missing-item      { who, item, count }                    — seller doesn't have the item
//
// Composition: shop reads/writes Inventory directly (atomic, no double-spend race).
// This is a hard dependency on the Inventory component — a shop without inventory makes no sense.
// The module does NOT emit pickup/drop events; it mutates Inventory synchronously.
// Sell price = floor(buy_price / 2), minimum 1. Items not in the shop catalog can't be sold.

// Helper: find the index of an item in a parallel-lists inventory, return {idx, count}.
function invFind(items, counts, item) {
  for (let i = 0; i < items.length; i++) {
    if (items[i] === item) {
      return { idx: i, count: counts[i] || 0 };
    }
  }
  return { idx: -1, count: 0 };
}

// Helper: remove count of an item from a parallel-lists inventory (mutates arrays in place).
// Returns true on success, false if insufficient.
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

// Helper: add count of an item to a parallel-lists inventory (mutates arrays in place).
// Returns true on success, false if inventory full (no existing slot + at capacity).
function invAdd(items, counts, item, count, capacity) {
  // Try to stack onto existing slot.
  for (let i = 0; i < items.length; i++) {
    if (items[i] === item) {
      counts[i] += count;
      return true;
    }
  }
  // No existing slot — check capacity.
  if (items.length >= capacity) return false;
  items.push(item);
  counts.push(count);
  return true;
}

vitric.fn("__shop_buy", (args, ctx) => {
  const who = args.who;
  const shop = args.shop;
  const item = String(args.item);
  const count = Number(args.count) || 1;
  if (!who) throw new Error("__shop_buy: missing who");
  if (!shop) throw new Error("__shop_buy: missing shop");
  if (!item) throw new Error("__shop_buy: missing item");
  if (count <= 0) throw new Error("__shop_buy: count must be > 0");

  // Read shop catalog.
  const shopItems = ctx.getField(shop, "Shop.items") || [];
  const shopPrices = ctx.getField(shop, "Shop.prices") || [];
  const shopStocks = ctx.getField(shop, "Shop.stocks") || [];
  const currency = ctx.getField(shop, "Shop.currency") || "coin";

  // Find item in catalog.
  const idx = shopItems.indexOf(item);
  if (idx < 0) {
    ctx.emit("shop-not-for-sale", { who, shop, item });
    return;
  }

  // Check stock (-1 = infinite).
  const stock = (idx < shopStocks.length && shopStocks[idx] !== undefined) ? shopStocks[idx] : -1;
  if (stock !== -1 && stock < count) {
    ctx.emit("shop-out-of-stock", { who, shop, item, available: stock });
    return;
  }

  // Check buyer's funds — read Inventory directly.
  const totalPrice = ((idx < shopPrices.length ? shopPrices[idx] : 0) * count);
  const invItems = ((ctx.getField(who, "Inventory.items") || [])).slice();
  const invCounts = ((ctx.getField(who, "Inventory.counts") || [])).slice();
  const cap = ctx.getField(who, "Inventory.capacity") || 16;

  if (totalPrice > 0) {
    const found = invFind(invItems, invCounts, currency);
    if (found.count < totalPrice) {
      ctx.emit("shop-insufficient-funds", { who, item, count, needed: totalPrice, have: found.count });
      return;
    }
    // Deduct currency.
    invRemove(invItems, invCounts, currency, totalPrice);
  }

  // Add purchased item.
  if (!invAdd(invItems, invCounts, item, count, cap)) {
    ctx.emit("shop-inventory-full", { who, item, count });
    return;
  }

  // Write back inventory (atomic — both currency removal and item addition in one write).
  ctx.setField(who, "Inventory.items", invItems);
  ctx.setField(who, "Inventory.counts", invCounts);

  // Decrement stock (if not infinite).
  if (stock !== -1) {
    const newStocks = shopStocks.slice();
    newStocks[idx] = stock - count;
    ctx.setField(shop, "Shop.stocks", newStocks);
  }

  ctx.emit("item-bought", { who, shop, item, count, total_price: totalPrice });
});

vitric.fn("__shop_sell", (args, ctx) => {
  const who = args.who;
  const shop = args.shop;
  const item = String(args.item);
  const count = Number(args.count) || 1;
  if (!who) throw new Error("__shop_sell: missing who");
  if (!shop) throw new Error("__shop_sell: missing shop");
  if (!item) throw new Error("__shop_sell: missing item");
  if (count <= 0) throw new Error("__shop_sell: count must be > 0");

  // Read shop catalog to determine sell price.
  const shopItems = ctx.getField(shop, "Shop.items") || [];
  const shopPrices = ctx.getField(shop, "Shop.prices") || [];
  const currency = ctx.getField(shop, "Shop.currency") || "coin";

  // Sell price = floor(buy_price / 2), minimum 1. Items not in catalog can't be sold.
  const idx = shopItems.indexOf(item);
  let unitPrice = 1;
  if (idx >= 0) {
    const buyPrice = (idx < shopPrices.length ? shopPrices[idx] : 0);
    unitPrice = Math.max(1, Math.floor(buyPrice / 2));
  } else {
    ctx.emit("shop-not-for-sale", { who, shop, item });
    return;
  }

  // Read seller's inventory.
  const invItems = ((ctx.getField(who, "Inventory.items") || [])).slice();
  const invCounts = ((ctx.getField(who, "Inventory.counts") || [])).slice();
  const cap = ctx.getField(who, "Inventory.capacity") || 16;

  // Remove the sold item.
  if (!invRemove(invItems, invCounts, item, count)) {
    ctx.emit("shop-missing-item", { who, item, count });
    return;
  }

  // Add currency.
  const totalPrice = unitPrice * count;
  invAdd(invItems, invCounts, currency, totalPrice, cap);

  // Write back inventory (atomic).
  ctx.setField(who, "Inventory.items", invItems);
  ctx.setField(who, "Inventory.counts", invCounts);

  ctx.emit("item-sold", { who, shop, item, count, total_price: totalPrice });
});
