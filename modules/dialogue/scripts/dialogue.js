// Dialogue module — branching dialogue tree driven by data + player choices.
//
// A dialogue tree lives on the NPC as a `Dialogue` component using parallel lists
// (lists-of-objects aren't a schema type, so each node is one entry across three
// parallel lists):
//
//   node_text[i]    — NPC's line at node i
//   node_choices[i] — player choice labels at node i, ";"-separated  (e.g. "Yes;No")
//   node_next[i]    — next node index per choice, ";"-separated      (e.g. "1;2")
//                     "-1" ends the dialogue
//   entry           — starting node index
//
// The player carries a `DialogueRunner` component (runtime state):
//   active_npc — entity name/handle of the NPC currently in conversation with
//   current    — current node index, -1 = not in a dialogue
//
// Flow:
//   1. Game emits `talk {npc, who}` (e.g. on collision with an NPC).
//   2. __dialogue_start sets runner.current = entry, emits dialogue-started.
//   3. Game renders the current node's text + choices (HUD). Player picks a choice;
//      game emits `dialogue-choose {who, choice_index}`.
//   4. __dialogue_choose reads node_next[current][choice_index]:
//        - if it's -1 (or missing) → __dialogue_end
//        - else → runner.current = next, emits dialogue-advanced
//   5. __dialogue_end clears the runner, increments the NPC's Talked.count (soft-depends
//      on the quest module's Talked component — skipped silently if absent), emits
//      dialogue-ended.
//
// Events emitted: dialogue-started / dialogue-advanced / dialogue-ended.
// The Talked.count increment is the composition seam with the quest module: a `talk`
// quest objective reads Talked.count > 0 and completes. So dialogue-end → quest progress.

// End the active dialogue on `who`: clear runner, bump NPC's Talked, emit ended.
function end_dialogue(who, ctx) {
  const npc = ctx.getField(who, "DialogueRunner.active_npc") || "";
  ctx.setField(who, "DialogueRunner.active_npc", "");
  ctx.setField(who, "DialogueRunner.current", -1);
  // Soft-depends on Talked (defined by the quest module, or the project schema).
  // If the NPC has no Talked component, skip the increment silently.
  const talked = ctx.getField(npc, "Talked.count");
  if (talked !== undefined && talked !== null) {
    ctx.setField(npc, "Talked.count", Number(talked) + 1);
  }
  ctx.emit("dialogue-ended", { who, npc });
}

// ---- inactive → in-dialogue (start at entry node) ----
vitric.fn("__dialogue_start", (args, ctx) => {
  const npc = args.npc;
  const who = args.who || "@player";
  if (!npc) throw new Error("__dialogue_start: 缺少 npc（实体名或句柄）");

  // Idempotent: if already in a dialogue, ignore further `talk` events.
  const current = ctx.getField(who, "DialogueRunner.current");
  if (current !== undefined && current !== null && current >= 0) return;

  const entry = ctx.getField(npc, "Dialogue.entry");
  if (entry === undefined || entry === null) return; // NPC has no Dialogue data

  ctx.setField(who, "DialogueRunner.active_npc", npc);
  ctx.setField(who, "DialogueRunner.current", entry);
  ctx.emit("dialogue-started", { npc, who, node: entry });
});

// ---- advance / end based on player's choice ----
vitric.fn("__dialogue_choose", (args, ctx) => {
  const who = args.who || "@player";
  const choiceIdx = Number(args.choice_index) || 0;

  const current = ctx.getField(who, "DialogueRunner.current");
  if (current === undefined || current === null || current < 0) return; // not in dialogue

  const npc = ctx.getField(who, "DialogueRunner.active_npc");
  if (!npc) return;

  const nodeNextList = ctx.getField(npc, "Dialogue.node_next") || [];
  const raw = nodeNextList[current];
  // No next data for this node → end the dialogue.
  if (raw === undefined || raw === null || raw === "") {
    end_dialogue(who, ctx);
    return;
  }

  const nexts = String(raw).split(";").map((s) => parseInt(s.trim(), 10));
  const next = nexts[choiceIdx];
  // -1, missing, or NaN → end the dialogue.
  if (next === undefined || isNaN(next) || next < 0) {
    end_dialogue(who, ctx);
    return;
  }

  ctx.setField(who, "DialogueRunner.current", next);
  ctx.emit("dialogue-advanced", { who, npc, node: next });
});

// ---- explicit end (e.g. game rule wants to force-close a dialogue) ----
vitric.fn("__dialogue_end", (args, ctx) => {
  const who = args.who || "@player";
  const current = ctx.getField(who, "DialogueRunner.current");
  if (current === undefined || current === null || current < 0) return; // not in dialogue
  end_dialogue(who, ctx);
});
