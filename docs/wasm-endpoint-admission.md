# Authenticated actions before WASM HTTP execution

An HttpEndpoint may declare `AdmissionActions`, a JSON-encoded array of ordered
OData bound actions. Before starting the WASM module the kernel runs each action
through the ordinary OData handler with the incoming request's authenticated user.
Cedar authorization, input validation and IOA state checks therefore apply before
external I/O. A rejection is returned directly; the module is not started.

Example field value:

```json
[{"name":"target","entity_set":"Targets","entity_id":"{id}","action":"Example.Serve"}]
```

The endpoint path supplies `{id}`. Captures are decoded once and escaped as OData
string values. Optional `params` maps parameter names to literals or entire capture
placeholders. The module receives successful action responses as `entity_state`,
keyed by `name`. These are request-local inputs, not new persisted entity fields.
No caller credential is passed to the module. Checks run on every request even
when an incoming idempotency key repeats. Ordered actions are not a transaction;
an earlier successful action is not rolled back if a later action rejects.

Endpoints without admission actions keep their existing behavior. Native transport
configuration and a separate admission list cannot be combined: malformed or
ambiguous configuration is rejected when constructing the route table.

## User and integration identity

Action-triggered integrations retain the caller's identity for local OData calls.
The kernel additionally supplies `context.module` naming the actual executing
module, overwriting any inherited value. This matches existing internal HTTP
credential behavior. It lets Cedar require both user ownership and the specific
integration. Direct WASM execution also supplies the module identity on local calls.

WASM HTTP endpoint modules continue making their own internal calls as the module;
the new pre-execution actions run as the incoming user. Existing protocol handlers
therefore retain their own module permissions.

Secret access remains separately authorized as the module, with the triggering
user available in the authorization context. Alice and Bob can trigger the same
integration using one webhook secret, without giving either user direct access
to that secret or allowing the integration to read unrelated secrets.

The compiled fixture in `crates/temper-server/tests/fixtures/wasm-identity` and
`wasm_identity_admission` tests exercise these boundaries with real WASM, Cedar,
OData and libSQL. The change contains no application-specific lifecycle rules.

## Standard-server reload regression

A real control-plane update exposed a separate startup defect. `serve --app`
restored passed verification from libSQL, loaded the same disk spec with a Pending
status, then skipped its background verification because the hash was cached as
verified. Reloading identical passed specs now retains the original evidence.
Changed, failed, running and pending specs still require verification.

Four CLI regressions cover this behavior. Before the fix, the two preservation
cases failed; all four pass afterwards. A real standard-server process started
twice against one libSQL database changed from `passed, pending` to `passed, passed`.
The full CLI suite passed 78 tests. This fixes normal application replacement; it
does not add control-plane recovery for interrupted integrations.
