# SDD Task Review Checklist

Mandatory checks for every task review. The reviewer sub-agent must verify each item
and include a compliance table in the review report. Any ❌ blocks task approval.

## 1. Schema field audit (CRITICAL)

The engine has an asymmetric tolerance for undeclared fields:
- JS system writes (`e.Comp.foo = x`) **tolerate** undeclared fields — no crash.
- Rule reads (`@entity.Comp.foo` in `set` / `if` / `format` args) **crash at runtime**
  when `foo` is not declared in `schema.json` under `components.<Comp>.fields`.

This asymmetry caused two regressions in the 2026-07-17 deepening (Task 4 and Task 8),
both only caught by Task 9's playthrough recording — not by review.

**Checklist** — for every new/modified `.js` system and `.json` rule in the task diff:

- [ ] Grep all `e.<Comp>.<field> =` writes in the diff's JS files. For each, confirm
      `components.<Comp>.fields.<field>` exists in `schema.json`. If missing → ❌.
- [ ] Grep all `@<entity>.<Comp>.<field>` reads in the diff's rule files. For each,
      confirm `components.<Comp>.fields.<field>` exists in `schema.json`. If missing → ❌.
- [ ] Grep all `ctx.getField(h, "<Comp>.<field>")` and `ctx.setField(h, "<Comp>.<field>", ...)`
      calls in the diff's JS files. For each, confirm the field is declared in `schema.json`. If missing → ❌.

If a field is genuinely intended as runtime-only and never read by a rule, the implementer
must still declare it in `schema.json` with a sensible type + default. Undeclared fields are
not allowed.

## 2. Enum variant audit

- [ ] For every `"set": "@<entity>.<EnumComp>.<value_field>", "to": "<literal>"` in the diff's
      rules, confirm `<literal>` is in `components.<EnumComp>.fields.<value_field>.variants`
      (or `enum_values`, whichever the schema uses). If missing → ❌.
- [ ] Same check for JS `ctx.setField(h, "<EnumComp>.<value_field>", "<literal>")` calls.

## 3. Scene entity reference audit

- [ ] For every `@<name>.<Comp>.<field>` in the diff's rules, confirm an entity named `<name>`
      exists in the game's scene file (`scenes/*.json` or `main.json`). If missing → ❌.
      Exception: rule `comment` fields may reference future entities if explicitly noted as
      "added in Task N" — but the rule must be a no-op until that entity exists (the engine
      silently fails `set` on missing entities, but this should be documented).

## 4. UI layout overlap audit (for scene edits)

When the task adds or moves a UI entity (`Ui` component with `anchor`/`ox`/`oy`/`w`/`h`):

- [ ] List all UI entities sharing the same `(anchor, parent)` pair.
- [ ] For each pair, compute the y-range `[oy, oy+h]` for every entity.
- [ ] Flag any overlapping ranges (touching at boundary is OK; interior overlap is a ❌).
- [ ] If overlap is intentional (e.g. a label on top of a panel), document it in the report.

## 5. Standard checks (existing)

- [ ] `cargo run -p vitric-cli -- check games/<game>` exits 0.
- [ ] All new `//` / `///` / `/* */` comments are in English (project convention).
- [ ] String literals (panic messages, game dialogue, UI labels) keep their authored language.
- [ ] No fake APIs (`ctx.singleton`, `ctx.each`, `vitric.on`, `vitric.expose`, `vitric.call`,
      `ctx.entity`, `ctx.llm`, `Math.random`). Use only verified real APIs.
- [ ] No dead code / YAGNI in new functions.
- [ ] Commit message follows `<type>(<scope>): <summary>` convention.
- [ ] Only in-scope files modified (per the task brief).

## How to run the audit quickly

```bash
# Schema field audit — collect all field references from the task diff
git diff <base>..HEAD -- 'games/frontier/scripts/*.js' 'games/frontier/rules/*.json' |
  grep -E '^\+' |
  grep -oE '(@|e\.)[A-Za-z_]+\.[A-Za-z_]+\.[A-Za-z_]+' |
  sort -u

# Then cross-check each against schema.json:
python3 -c "
import json
s = json.load(open('games/frontier/schema.json'))
fields = set()
for comp, spec in s['components'].items():
    for f in spec.get('fields', {}):
        fields.add(f'{comp}.{f}')
for f in sorted(fields):
    print(f)
"
```
