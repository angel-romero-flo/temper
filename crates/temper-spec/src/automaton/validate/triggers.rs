//! `[[action.triggers]]` and `[[webhook]]` checks (ADR-0046, ADR-0181).

use std::collections::BTreeSet;

use super::super::parser::AutomatonParseError;
use super::super::types::*;
use crate::predicate::Arg;

/// Names an `args` value may read: declared state variables and the id.
fn state_names(automaton: &Automaton) -> BTreeSet<&str> {
    automaton
        .state
        .iter()
        .map(|sv| sv.name.as_str())
        .chain(["Id", "id"])
        .collect()
}

/// Validate all `[[action.triggers]]` blocks per ADR-0046 rules.
///
/// Checks performed (parse-time, per-entity — cross-entity checks like
/// target-action existence happen at registry load time):
/// - Kind-specific required fields present, and no field of another kind.
/// - The statuses the guard pins are declared states.
/// - Each `args` value is a literal or a declared state variable, `Id`, or a
///   parameter of the source action (parameters are stored as fields).
/// - For `Wasm`/`Adapter`/`Webhook` kinds: `on_success`/`on_failure` reference
///   actions declared on the same source entity.
/// - Trigger names within a single action must be unique.
pub(super) fn validate_action_triggers(
    automaton: &Automaton,
    action_names: &[&str],
) -> Result<(), AutomatonParseError> {
    let state_names = state_names(automaton);
    for action in &automaton.actions {
        let mut seen_names: BTreeSet<&str> = BTreeSet::new();
        for trigger in &action.triggers {
            if trigger.name.is_empty() {
                return Err(AutomatonParseError::Validation(format!(
                    "action '{}' has an [[action.triggers]] block with empty name",
                    action.name
                )));
            }
            if !seen_names.insert(trigger.name.as_str()) {
                return Err(AutomatonParseError::Validation(format!(
                    "action '{}' declares trigger '{}' more than once",
                    action.name, trigger.name
                )));
            }

            // Kind-specific field presence.
            match trigger.kind {
                TriggerKind::Entity => {
                    if trigger.target_entity.as_deref().is_none_or(str::is_empty) {
                        return Err(AutomatonParseError::Validation(format!(
                            "trigger '{}' on action '{}' is kind=\"entity\" but missing 'target_entity'",
                            trigger.name, action.name
                        )));
                    }
                    if trigger.target_action.as_deref().is_none_or(str::is_empty) {
                        return Err(AutomatonParseError::Validation(format!(
                            "trigger '{}' on action '{}' is kind=\"entity\" but missing 'target_action'",
                            trigger.name, action.name
                        )));
                    }
                    if trigger.resolve_target.is_none() {
                        return Err(AutomatonParseError::Validation(format!(
                            "trigger '{}' on action '{}' is kind=\"entity\" but missing 'resolve_target'",
                            trigger.name, action.name
                        )));
                    }
                }
                TriggerKind::Wasm => {
                    if trigger.module.as_deref().is_none_or(str::is_empty) {
                        return Err(AutomatonParseError::Validation(format!(
                            "trigger '{}' on action '{}' is kind=\"wasm\" but missing 'module'",
                            trigger.name, action.name
                        )));
                    }
                }
                TriggerKind::Adapter => {
                    if trigger
                        .adapter
                        .as_deref()
                        .is_none_or(|a| a.trim().is_empty())
                    {
                        return Err(AutomatonParseError::Validation(format!(
                            "trigger '{}' on action '{}' is kind=\"adapter\" but missing 'adapter'",
                            trigger.name, action.name
                        )));
                    }
                }
                TriggerKind::Hook => {
                    if trigger.hook.as_deref().is_none_or(str::is_empty) {
                        return Err(AutomatonParseError::Validation(format!(
                            "trigger '{}' on action '{}' is kind=\"hook\" but missing 'hook'",
                            trigger.name, action.name
                        )));
                    }
                }
                TriggerKind::Webhook => {
                    if trigger.url.as_deref().is_none_or(str::is_empty) {
                        return Err(AutomatonParseError::Validation(format!(
                            "trigger '{}' on action '{}' is kind=\"webhook\" but missing 'url'",
                            trigger.name, action.name
                        )));
                    }
                    if trigger.method.as_deref().is_none_or(str::is_empty) {
                        return Err(AutomatonParseError::Validation(format!(
                            "trigger '{}' on action '{}' is kind=\"webhook\" but missing 'method'",
                            trigger.name, action.name
                        )));
                    }
                }
            }

            let scope = format!("trigger '{}' on action '{}'", trigger.name, action.name);
            if let Some(key) = trigger.foreign_keys().first() {
                return Err(AutomatonParseError::Validation(format!(
                    "{scope}: `{key}` does not apply to kind = \"{}\"",
                    trigger.kind.as_str()
                )));
            }
            for status in trigger.status_filter().unwrap_or_default() {
                if !automaton.automaton.states.contains(&status) {
                    return Err(AutomatonParseError::Validation(format!(
                        "{scope} guard: '{status}' is not a declared state"
                    )));
                }
            }
            let mut readable: BTreeSet<&str> = state_names.clone();
            readable.extend(action.params.iter().map(ActionParam::name));
            check_args(&format!("{scope} args"), &trigger.args, &readable)?;

            // on_success / on_failure must reference actions declared on this
            // source entity (they dispatch on the source after module/HTTP).
            if let Some(ref cb) = trigger.on_success
                && !action_names.contains(&cb.as_str())
            {
                return Err(AutomatonParseError::Validation(format!(
                    "trigger '{}' on action '{}' on_success references unknown action '{cb}'",
                    trigger.name, action.name
                )));
            }
            if let Some(ref cb) = trigger.on_failure
                && !action_names.contains(&cb.as_str())
            {
                return Err(AutomatonParseError::Validation(format!(
                    "trigger '{}' on action '{}' on_failure references unknown action '{cb}'",
                    trigger.name, action.name
                )));
            }
        }
    }

    Ok(())
}

/// Each value is a literal or a name in `readable`.
fn check_args(
    scope: &str,
    args: &std::collections::BTreeMap<String, Arg>,
    readable: &BTreeSet<&str>,
) -> Result<(), AutomatonParseError> {
    for (key, arg) in args {
        match arg {
            Arg::Lit(_) => {}
            Arg::Var(name) if readable.contains(name.as_str()) => {}
            Arg::Var(name) => {
                return Err(AutomatonParseError::Validation(format!(
                    "{scope}: {key} = {name} reads no declared state variable or parameter; \
                     write a string literal in single quotes, such as '{name}'"
                )));
            }
            Arg::Param(name) => {
                return Err(AutomatonParseError::Validation(format!(
                    "{scope}: {key} = params.{name}: args read parameters by name, write {name}"
                )));
            }
        }
    }
    Ok(())
}

/// `[[state_timeout]] args` read declared state variables, and `[[webhook]]`
/// sources are `query.<name>`.
pub(super) fn validate_timeouts_and_webhooks(
    automaton: &Automaton,
) -> Result<(), AutomatonParseError> {
    let state_names = state_names(automaton);
    for st in &automaton.state_timeouts {
        check_args(
            &format!("state_timeout for '{}' args", st.state),
            &st.args,
            &state_names,
        )?;
    }
    for webhook in &automaton.webhooks {
        let sources = std::iter::once(("entity_id", &webhook.entity_id))
            .chain(webhook.extract.iter().map(|(k, v)| (k.as_str(), v)));
        for (key, source) in sources {
            if Webhook::query_param(source).is_none() {
                return Err(AutomatonParseError::Validation(format!(
                    "webhook '{}': {key} = \"{source}\" must be query.<name> (the query string is the only source)",
                    webhook.name
                )));
            }
        }
    }
    Ok(())
}
