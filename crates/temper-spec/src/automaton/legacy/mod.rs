//! The syntax that preceded [`crate::predicate`]: guard strings and tables,
//! `assert` + `when`, trigger-guard tables, field-predicate tables, verb and
//! table effects, `[[integration]]` blocks, and the value spellings and key
//! names that preceded strict typed reading (string booleans and numbers,
//! `to_state`, `params`/`params_from`, ...). Kept only to convert old specs
//! to the current syntax ([`migrate_source`]).

mod assert_parser;
mod effects;
mod field_predicate;
mod guard_syntax;
mod lower;
mod migrate;
mod migrate_effects;
mod migrate_names;
mod syntax;

pub use lower::{LoweredInvariant, invariant_to_expr};
pub use migrate::{Migration, migrate_source, simplify};

/// Whether `source` is written in a syntax `migrate_source` converts: it
/// parses as TOML and a conversion pass would change it.
pub(crate) fn uses_old_syntax(source: &str) -> bool {
    migrate::rewrite(source, false).is_ok_and(|(rewritten, _)| rewritten != source)
}

/// Render a scalar as the string the spec author wrote. `None` for arrays
/// and tables, which have no scalar spelling.
pub(crate) fn scalar_string(value: &toml::Value) -> Option<String> {
    use toml::Value;
    match value {
        Value::String(s) => Some(s.clone()),
        Value::Integer(i) => Some(i.to_string()),
        Value::Float(f) => Some(f.to_string()),
        Value::Boolean(b) => Some(b.to_string()),
        Value::Datetime(d) => Some(d.to_string()),
        Value::Array(_) | Value::Table(_) => None,
    }
}

/// Render any value as a string: scalars as written, arrays and tables as TOML.
pub(crate) fn any_string(value: &toml::Value) -> String {
    scalar_string(value).unwrap_or_else(|| value.to_string())
}

/// Read a non-negative integer written as a number or a numeric string.
pub(crate) fn unsigned<T: std::str::FromStr>(table: &toml::Table, key: &str) -> Option<T> {
    table.get(key).and_then(scalar_string)?.parse().ok()
}

/// Whether `value` is an action guard in the old syntax (a clause such as
/// `"is_true x"`, a `{ type = ... }` table, or an array of either).
pub(crate) fn is_legacy_guard(value: &toml::Value) -> bool {
    let mut guards = Vec::new();
    guard_syntax::parse_guard_value(value, &mut guards).is_ok()
}

/// Whether `value` is an action `effect` in the old syntax.
pub(crate) fn is_legacy_effect(value: &toml::Value) -> bool {
    let mut effects = Vec::new();
    effects::parse_effect_value(value, &mut effects).is_ok() && !effects.is_empty()
}

#[cfg(test)]
#[path = "lower_test.rs"]
mod tests;
