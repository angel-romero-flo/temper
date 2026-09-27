# ADR-0181: Strict typed spec reading; one name per concept

- Status: Accepted
- Date: 2026-09-27
- Deciders: Temper core maintainers
- Related:
  - ADR-0179: One predicate grammar for every spec condition (the grammar `args` values use)
  - ADR-0180: Effects as statements; triggers as the only outgoing call
  - ADR-0046: Unified action triggers (`to_state`, `params`, `params_from`, `resolve_target`)
  - ADR-0049: State timeouts (`params` becomes `args`)
  - ADR-0040: Composite actions (`record_parent_event`, `cedar_gate`, `sub_writes`)
  - Issue #500 (audit section D, proposal, decisions and implementation plan)
  - `crates/temper-spec/src/automaton/toml_parser/`, `crates/temper-spec/src/automaton/types/`, `crates/temper-spec/src/automaton/legacy/migrate_names.rs`

## Context

After ADR-0179 and ADR-0180 a spec's conditions and effects each had one syntax, but its values and names did not:

1. **Lenient values.** The core sections (`[automaton]`, `[[state]]`, `[[action]]`, `[[liveness]]`) were read key by key: unknown keys were ignored, `"true"` and `"0"` were accepted as a boolean and an integer, and a bad value on most keys silently became the default (`record_parent_event` read as `true`, `query_indexed` and `overflow_*` were dropped, `has_actions` read as `false`). An entry without a `name` was skipped. `[automaton.timeouts]` in two temper-agents specs was read by nothing.
2. **String initials parsed by each consumer.** `[[state]] initial` was a string that the JIT, verifier and actor runtime each parsed: a boolean accepted `yes`, `on` and `1`, a bad counter became `0`, and the actor runtime ignored list initials.
3. **Two type vocabularies.** `[[state]] type` accepted ten names and action params seven, overlapping in part (`uint64` only for params, `set`/`status`/`float`/`number` only for state), string-matched in six places with different fallbacks.
4. **`kind` unchecked.** Any action `kind` loaded (`"inptu"` was an internal action nothing subscribed to), and `Composite` was the only capitalised value, compared case-insensitively in three places.
5. **One key, two meanings; one concept, two keys.** `params` declared an action's inputs but supplied a trigger's or timeout's arguments (with `params_from` for arguments read from fields). Trigger `to_state` duplicated what the post-state `guard` expresses. The field holding another entity's id was `field` in one resolver and `id_field` in another. `resolve_target` was tagged with `type`, which elsewhere means a data type. Webhook `entity_lookup` was never read. Triggers accepted both `adapter` and `adapter_type`.

## Decision

### Sub-Decision 1: every section is typed and strict

Every section deserializes through serde into its type with `deny_unknown_fields`; the lenient reader (`toml_parser/values.rs`) is deleted. Booleans, integers and lists use TOML's own types. A bad value, an unknown key or section, or an entry without a `name` is a load error naming the section, the entry and the key (`state 'count': initial: a counter starts at a non-negative integer, such as ``initial = 0``, found "0"`). Retired keys (`to_state`, `params`, `params_from`, `adapter_type`, `resolve_target.type`/`field`, `entity_lookup`/`entity_param`, `[automaton.timeouts]`) get an error saying what to write instead, and a spec the converter would change gets a pointer to `temper migrate-predicates`.

### Sub-Decision 2: one type vocabulary

`[[state]] type` and action param `type` share one enum, `VarType`: `string`, `bool`, `counter` (non-negative, modeled), `int` (signed 64-bit, not modeled), `list` (of strings). `status`, `set`, `integer`, `float`, `number` and `uint64` are removed. `initial` is required and written in the type's TOML form (`initial = 0`, `initial = false`, `initial = []`, `initial = ""`); it is read once into a typed `Initial`, and consumers match on it exhaustively.

### Sub-Decision 3: `type` is a data type, `kind` is a variant

`type` appears only on `[[state]]` and params. Sections with variants are tagged with `kind` from a closed, lowercase set checked at load: actions (`input`, `internal`, `output`, `composite`, an `ActionKind` enum), triggers (`entity`, `wasm`, `adapter`, `webhook`, `hook`), constraints, and `resolve_target` (`field`, `same_id`, `static`, `create_if_missing`, `create`). `record_parent_event`, `cedar_gate` and `sub_writes` are only allowed on `composite` actions; a trigger may only set the keys its kind reads (a `guard`, `principal`, `liveness` or `drop_ok` only on entity triggers, since the other dispatchers never read them).

### Sub-Decision 4: one name per concept

- **`args`** replaces trigger `params` + `params_from` and state-timeout `params`: one table whose values are literals (`'text'`, integers, `true`, `false`, `null`) or names in the predicate grammar. A name is a field of the entity (`Id` is its id), read after the source commits (triggers) or when the timer is armed (timeouts); it must be a declared state variable, `Id`, or (triggers) a parameter of the source action, so a string written without its quotes fails to load. `params` now only declares an action's inputs.
- **`to_state`** is removed; the trigger `guard` (already over the post-state) carries it as `status == 'S'`. The registry, trigger graph and composite verifier derive the statuses a trigger can fire in from the guard's top-level `status ==` / `status in` conjuncts (`Expr::required_statuses`).
- **`id_field`** names the field holding another entity's id in every resolver.
- **Webhook `entity_id = "query.<name>"`** replaces `entity_lookup` + `entity_param`, in the same source syntax as `extract`; `query.` is the only source.
- **`adapter`** is the only adapter key.

**Why:** a spec is an agent's contract with the kernel. A value the kernel reads differently from how it was written, or ignores, is a silent failure; a closed vocabulary with typed values turns each into a load error with the fix in the message.

## Rollout Plan

1. **This change:** the strict reader, the types, the renames, and a third `temper migrate-predicates` pass (`legacy/migrate_names.rs`) that rewrites old spellings and names in place and drops, with a note, keys the old reader ignored. Every spec in the repo is converted; the two `[automaton.timeouts]` blocks are dropped (nothing read them).
2. **Stored tenant specs** follow ADR-0179: run `temper migrate-predicates` before starting this version. One run applies all three passes.
3. **TemperPaw** converts its specs when it bumps its pinned `temper`.

## Consequences

### Positive
- A spec loads with the meaning it was written with, or fails with the key and the fix.
- One type vocabulary and one typed initial value across the JIT, verifier and runtimes; the actor runtime now honours list initials.
- Each concept has one name; `params` means one thing.

### Negative
- Breaking: specs outside this repo must be converted (`temper migrate-predicates`).
- `args` string literals cannot contain `'` (the grammar has no escapes); no current literal does.

### Risks
- A converted spec that relied on an ignored key loses it silently at runtime. Mitigation: the converter prints a note for every key it drops, and a differential snapshot showed all 131 repo specs and 110 TemperPaw specs parse to the same meaning after conversion.

### DST Compliance
- `args` resolve in one pure function (`automaton::resolve_args`) shared by the async dispatcher, the sim dispatcher and state timeouts; map iteration is `BTreeMap`.

## Non-Goals

- Aligning entity-type and action key names across triggers, `sub_writes` and `[[context_entity]]`.
- `[[action.constraints]]` and `[[cross_invariant]]` (ADR-0179 non-goals).
- Escapes in string literals.

## Alternatives Considered

1. **`args` as two maps** (TOML literals in `args`, field names in `args_from`) — keeps literals unquoted, but keeps two keys for one concept.
2. **Optional `initial` with type defaults** — about 1,700 fewer lines, but a reader has to know the defaults.
3. **Converting `[automaton.timeouts]` to `[[state_timeout]]`** — matches the obvious intent, but turns on timeouts that never fired; dropped instead.
4. **Keeping `[[action.triggers]]` a superset without per-kind checks** — lets a `guard` on a wasm trigger load and never run.
