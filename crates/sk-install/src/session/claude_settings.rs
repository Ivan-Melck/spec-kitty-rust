//! The ONE Source `json.loads`-backed `.claude/settings.json` model.
//!
//! The reference (`src/specify_cli/session_presence/hooks/claude_code_hook.py`)
//! reads the settings file with `json.loads` and then asks one question — "is
//! this command registered for this lifecycle event" — with
//! `_event_entries` → `_iter_command_hooks` → `hook.get("type") == "command"
//! and hook.get("command") == command`. `json.loads` accepts documents that a
//! strict JSON writer refuses, so those values are part of the model:
//!
//! * bare `NaN`, `Infinity` and `-Infinity` constants (and float overflow such
//!   as `1e400`, which is `inf`);
//! * arbitrarily large integers, up to the interpreter's
//!   `sys.get_int_max_str_digits()` ceiling;
//! * exact Python strings, including lone surrogate units from `\uXXXX`
//!   escapes, as **values and as object keys**;
//! * duplicate keys (last value wins at the first position) and any member
//!   order.
//!
//! The model is built on the workspace's one exact JSON authority
//! ([`sk_kernel::python_json`]): [`PythonJsonValue`], [`PythonJsonDict`] and
//! [`PythonString`] with [`parse_python_string`] as the reader and
//! [`PythonJsonValue::render_python`] as the writer. There is no second parser,
//! no `serde_json` view in this module, no stringify/reparse bridge and no
//! scalar-dropping round trip.
//!
//! # Failure phases (`_load`'s catch)
//!
//! `_load` catches `json.JSONDecodeError` only, and also treats a non-dict
//! value as `{}`. Every refusal in [`ClaudeSettingsDocument::InvalidJson`] /
//! [`ClaudeSettingsDocument::NotObject`] is therefore "no hooks", not an error.
//! The integer ceiling (`ValueError`) and the recursion ceiling
//! (`RecursionError`) are **not** `JSONDecodeError`s: the reference has no
//! catch for them, so they escape the provider and stay
//! [`ClaudeSettingsDocument::Uncaught`] here instead of silently becoming
//! "hooks missing".

use sk_kernel::PythonString;
use sk_kernel::python_json::{
    PythonJsonDict, PythonJsonError, PythonJsonErrorKind, PythonJsonValue, parse_python_string,
};

/// A `json.loads` failure class the reference `_load` does not catch.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClaudeSettingsLimit {
    /// CPython's `sys.get_int_max_str_digits()` conversion ceiling
    /// (`ValueError`, no position).
    IntegerDigits,
    /// CPython's recursion ceiling (`RecursionError`).
    RecursionDepth,
}

/// A `json.loads` failure the reference lets escape `_load`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClaudeSettingsFailure {
    limit: Option<ClaudeSettingsLimit>,
    detail: String,
}

impl ClaudeSettingsFailure {
    /// The Source limit class, when the failure is one of CPython's two JSON
    /// ceilings. `None` is a native-only refusal (for example a platform float
    /// parser failure or an interpreter-startup setting that no JSON phase
    /// reaches) and is reported as such rather than labelled a Source format
    /// rule.
    pub fn limit(&self) -> Option<ClaudeSettingsLimit> {
        self.limit
    }

    /// The refusal text (the Source message verbatim where one exists).
    pub fn detail(&self) -> &str {
        &self.detail
    }
}

/// The reference `_load` result for one already-decoded document.
#[derive(Clone, Debug, PartialEq)]
pub enum ClaudeSettingsDocument {
    /// `json.loads` returned a `dict`: the document `_load` returns.
    Object(PythonJsonDict),
    /// `json.loads` raised `json.JSONDecodeError`, so `_load` returned `{}`.
    /// The `JSONDecodeError` text is carried for diagnostics.
    InvalidJson(String),
    /// `json.loads` returned a non-dict value, so `_load` returned `{}`.
    NotObject,
    /// `json.loads` raised something `_load` does not catch.
    Uncaught(ClaudeSettingsFailure),
}

/// Parse one decoded settings document exactly as the reference `_load` does.
pub fn parse_settings(text: &PythonString) -> ClaudeSettingsDocument {
    match parse_python_string(text) {
        Ok(PythonJsonValue::ExactObject(object)) => ClaudeSettingsDocument::Object(object),
        Ok(_) => ClaudeSettingsDocument::NotObject,
        Err(error) => classify_error(error),
    }
}

/// Map one Kernel reader refusal to the reference `_load` phase.
fn classify_error(error: PythonJsonError) -> ClaudeSettingsDocument {
    match error.kind() {
        PythonJsonErrorKind::IntegerDigitLimit => {
            ClaudeSettingsDocument::Uncaught(ClaudeSettingsFailure {
                limit: Some(ClaudeSettingsLimit::IntegerDigits),
                detail: error.message(),
            })
        }
        PythonJsonErrorKind::RecursionBudget => {
            ClaudeSettingsDocument::Uncaught(ClaudeSettingsFailure {
                limit: Some(ClaudeSettingsLimit::RecursionDepth),
                detail: error.message(),
            })
        }
        PythonJsonErrorKind::IntegerDigitLimitSetting | PythonJsonErrorKind::NumberConversion => {
            ClaudeSettingsDocument::Uncaught(ClaudeSettingsFailure {
                limit: None,
                detail: error.message(),
            })
        }
        // Every other kind is a `json.JSONDecodeError` in the reference.
        PythonJsonErrorKind::ExpectingValue
        | PythonJsonErrorKind::ExpectingDelimiter
        | PythonJsonErrorKind::ExpectingColon
        | PythonJsonErrorKind::ExpectingPropertyName
        | PythonJsonErrorKind::ExtraData
        | PythonJsonErrorKind::UnterminatedString
        | PythonJsonErrorKind::InvalidEscape
        | PythonJsonErrorKind::InvalidUnicodeEscape
        | PythonJsonErrorKind::InvalidControlCharacter
        | PythonJsonErrorKind::UnexpectedBom => {
            ClaudeSettingsDocument::InvalidJson(error.to_string())
        }
    }
}

/// Exact Python `str` equality between a decoded member and Rust text.
fn is_text(value: Option<&PythonJsonValue>, text: &str) -> bool {
    matches!(
        value,
        Some(PythonJsonValue::String(value))
            if value.codepoints().iter().copied().eq(text.chars().map(u32::from))
    )
}

/// An object member, for either decoded object representation.
fn member<'a>(object: &'a PythonJsonDict, key: &str) -> Option<&'a PythonJsonValue> {
    object.get(key)
}

/// Source `_event_entries`: `hooks` must be a dict and the event a list.
fn event_entries<'a>(document: &'a PythonJsonDict, event: &str) -> Option<&'a [PythonJsonValue]> {
    let PythonJsonValue::ExactObject(hooks) = member(document, "hooks")? else {
        return None;
    };
    match member(hooks, event)? {
        PythonJsonValue::Array(entries) => Some(entries),
        _ => None,
    }
}

/// Source `_iter_command_hooks`: the dict hooks of every entry group.
fn command_hooks<'a>(
    entries: &'a [PythonJsonValue],
) -> impl Iterator<Item = &'a PythonJsonDict> + 'a {
    entries
        .iter()
        .filter_map(|entry| {
            let PythonJsonValue::ExactObject(group) = entry else {
                return None;
            };
            let PythonJsonValue::Array(hooks) = member(group, "hooks")? else {
                return None;
            };
            Some(hooks.iter().filter_map(|hook| match hook {
                PythonJsonValue::ExactObject(hook) => Some(hook),
                _ => None,
            }))
        })
        .flatten()
}

/// Whether one entry group list runs `command` as a `type: command` hook.
fn entries_run(entries: &[PythonJsonValue], command: &str) -> bool {
    command_hooks(entries).any(|hook| {
        is_text(member(hook, "type"), "command") && is_text(member(hook, "command"), command)
    })
}

/// Source `is_registered` for one lifecycle event.
pub fn registered(document: &PythonJsonDict, event: &str, command: &str) -> bool {
    event_entries(document, event).is_some_and(|entries| entries_run(entries, command))
}

/// Source `_hooks_registered`: every `(event, command)` pair is registered.
pub fn registered_all(document: &PythonJsonDict, pairs: &[(&str, &str)]) -> bool {
    pairs
        .iter()
        .all(|(event, command)| registered(document, event, command))
}

/// The reference's "this event also carries entries that are not ours" fact.
///
/// Used only by the read-only assessment; the merge below never rewrites the
/// entries it reports. A non-array event value is *absent* (not foreign),
/// matching `_event_entries`; a malformed group or hook inside an array is
/// foreign, matching the writer's preserve posture.
pub fn foreign_entries(document: &PythonJsonDict, pairs: &[(&str, &str)]) -> bool {
    let Some(PythonJsonValue::ExactObject(hooks)) = member(document, "hooks") else {
        return false;
    };
    for (event, command) in pairs {
        let Some(PythonJsonValue::Array(entries)) = member(hooks, event) else {
            continue;
        };
        for entry in entries {
            let PythonJsonValue::ExactObject(group) = entry else {
                return true;
            };
            let Some(PythonJsonValue::Array(entry_hooks)) = member(group, "hooks") else {
                return true;
            };
            for hook in entry_hooks {
                let PythonJsonValue::ExactObject(hook) = hook else {
                    return true;
                };
                let ours = is_text(member(hook, "type"), "command")
                    && is_text(member(hook, "command"), command);
                if !ours {
                    return true;
                }
            }
        }
    }
    false
}

/// What the reference merge does to one document.
#[derive(Clone, Debug, PartialEq)]
pub enum ClaudeSettingsMerge {
    /// Every command is already registered: the reference writes nothing.
    Unchanged,
    /// The document (with the missing entries appended) the reference writes.
    Merged(PythonJsonDict),
    /// `hooks`, or the event value, is not the shape the reference replaces.
    /// The reference raises `ValueError` here (`_validate_hook_entries`) and the
    /// writer leaves the file untouched instead of repairing it.
    NotMergeable,
}

/// Append the missing `(event, command)` entries exactly as the reference does.
///
/// `json.loads` → `dict` insertion order, `setdefault` appends the `hooks`
/// section and each event at the end, and a new entry is
/// `{"hooks": [{"type": "command", "command": <command>}]}`.
pub fn merge_required(document: PythonJsonDict, pairs: &[(&str, &str)]) -> ClaudeSettingsMerge {
    let mut merged = document;
    let mut hooks = match member(&merged, "hooks") {
        Some(PythonJsonValue::ExactObject(hooks)) => hooks.clone(),
        Some(_) => return ClaudeSettingsMerge::NotMergeable,
        None => PythonJsonDict::new(),
    };
    let mut changed = false;
    for (event, command) in pairs {
        let mut entries = match member(&hooks, event) {
            Some(PythonJsonValue::Array(entries)) => entries.clone(),
            Some(_) => return ClaudeSettingsMerge::NotMergeable,
            None => Vec::new(),
        };
        if !entries_run(&entries, command) {
            entries.push(command_entry(command));
            hooks.insert(
                PythonString::from_scalars(event),
                PythonJsonValue::Array(entries),
            );
            changed = true;
        }
    }
    if !changed {
        return ClaudeSettingsMerge::Unchanged;
    }
    merged.insert(
        PythonString::from_scalars("hooks"),
        PythonJsonValue::ExactObject(hooks),
    );
    ClaudeSettingsMerge::Merged(merged)
}

/// The reference's appended hook group for one command.
fn command_entry(command: &str) -> PythonJsonValue {
    let mut hook = PythonJsonDict::new();
    hook.insert(
        PythonString::from_scalars("type"),
        PythonJsonValue::scalar("command"),
    );
    hook.insert(
        PythonString::from_scalars("command"),
        PythonJsonValue::scalar(command),
    );
    let mut group = PythonJsonDict::new();
    group.insert(
        PythonString::from_scalars("hooks"),
        PythonJsonValue::Array(vec![PythonJsonValue::ExactObject(hook)]),
    );
    PythonJsonValue::ExactObject(group)
}

/// Render one document exactly as `json.dumps(document, indent=2) + "\n"`.
///
/// `ensure_ascii=True` is the reference default, so the result is always ASCII
/// (a lone surrogate is emitted as `\udXXX`) and never needs a lossy encode.
pub fn render_settings(document: &PythonJsonDict) -> String {
    let mut rendered = PythonJsonValue::ExactObject(document.clone()).render(Some(2));
    rendered.push('\n');
    rendered
}
