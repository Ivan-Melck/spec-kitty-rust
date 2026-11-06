//! `charter context --include` — the CLI half of the fetch-selector surface.
//!
//! The reference CLI (`specify_cli.cli.commands.charter.context`) prints the rendered
//! selector verbatim (Rich with `markup=False`), wraps it in
//! `{"result": "success", "success": true, "include": …, "context": …, "text": …}` for
//! `--json`, and lets a `ValueError` become `Error: <message>` (exit 1). The selector
//! grammar, kind vocabulary, identity resolution, merged-record serving, effective-deny
//! annotation and body projection live in `sk-doctrine`'s `context_include` module; this
//! file only wires them to the CLI's project root, the canonical lazy access authority
//! (`IncludeAccess`: one identity index plus a deferred offering service, never the whole
//! doctrine bundle) and the two output envelopes.
//!
//! The rendered value is [`sk_doctrine::context_include::render_include_exact`]: exact code
//! points, not a scalar projection. The reference's stdout is CPython's
//! `sys.stdout` `TextIOWrapper`, so both arms go through
//! [`crate::compat::python_output`] — the ONE composition of the startup
//! encoding/error policy (`PYTHONIOENCODING`, UTF-8 mode, real locale codeset).
//! Text mode therefore emits exactly what the reference emits for that policy
//! (for example a `\udc80` unit becomes the raw byte `0x80` under the default
//! `surrogateescape`, and `utf-8:backslashreplace` prints `\udc80`); a unit the
//! active policy cannot encode is a distinct late refusal, never a substituted
//! character and never a partial write. `--json` renders the reference envelope
//! with the ONE Kernel `json.dumps` encoder (`indent=2`, `ensure_ascii=True`,
//! reference key order), so the DOCUMENT is ASCII exactly like the reference —
//! while the same startup policy still decides the PHYSICAL bytes (for example
//! UTF-16 when `PYTHONIOENCODING=utf-16`).
//!
//! `section:<slug>` is rendered here from `charter.md`, exactly as before.
//!
//! `template:<mission>/<name>` takes a narrow fast path: after the shared selector
//! grammar it delegates straight to the ONE canonical template resolver/include owner in
//! `sk_charter::template_catalog`, so a template read never constructs the doctrine
//! bundle (artifact graph, identity index, profile repository) it cannot need. The two
//! envelopes below are the same helpers the other kinds use.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use sk_charter::template_catalog::{TemplateTier, render_source_template_include};
use sk_doctrine::context_include::{
    IncludeAccess, IncludeAccessError, IncludeError, IncludeInputs, IncludeSelector,
    render_include_exact,
};
use sk_kernel::PythonString;
use sk_kernel::python_json::{PythonJsonDict, PythonJsonRenderOptions, PythonJsonValue};

use crate::compat::python_output::{
    PYTHONIOENCODING_HINT, StdoutError, write_source_json, write_source_text,
};
use crate::compat::support::CompatError;

/// Whether this invocation already printed the legacy-tier migrate nudge.
static LEGACY_NUDGE_SHOWN: AtomicBool = AtomicBool::new(false);

/// Render one `--include` selector for `root` and return the process exit code.
pub(super) fn render(root: &Path, include: &str, json_output: bool) -> Result<u8, CompatError> {
    let selector =
        sk_doctrine::context_include::parse_include_selector(include).map_err(selector_error)?;
    if selector.kind == "section" {
        return section(root, include, &selector, json_output);
    }
    if selector.kind == "template" {
        return template(root, include, &selector, json_output);
    }
    // Source builds only the ledger/identity needed for the deny annotation, then the
    // offering service and the queried repository. `IncludeAccess` defers both, so a
    // `directive` include never parses the profile catalogue, the DRG bundle or an
    // unrelated artifact kind.
    let access = IncludeAccess::load(root).map_err(access_error)?;
    let inputs = IncludeInputs {
        project_root: root,
        access: &access,
    };
    let text = render_include_exact(&inputs, &selector, include).map_err(selector_error)?;
    emit(include, &text, json_output)
}

/// Render the `template:<mission>/<name>` selector without building a doctrine bundle.
fn template(
    root: &Path,
    include: &str,
    selector: &IncludeSelector,
    json_output: bool,
) -> Result<u8, CompatError> {
    match render_source_template_include(root, &selector.identifier, include) {
        Ok(rendered) => {
            if rendered.tier == TemplateTier::Legacy {
                legacy_tier_nudge(json_output);
            }
            emit(
                include,
                &PythonString::from_scalars(&rendered.text),
                json_output,
            )
        }
        Err(error) => Err(refusal(error.to_string())),
    }
}

/// Print the reference's one-time "run `spec-kitty migrate`" nudge for a legacy hit.
///
/// The reference prints it only when the global runtime is already configured
/// (`~/.kittify/cache/version.lock` exists) and never in `--json` mode, so machine output
/// stays a single JSON object. Its Python `logging.warning`/`DeprecationWarning` line for
/// the un-configured case is NOT reproduced (see `sk-charter`'s module note); nothing here
/// invents Python provenance text.
fn legacy_tier_nudge(json_output: bool) {
    if json_output {
        return;
    }
    let Ok(home) = sk_kernel::get_kittify_home() else {
        return;
    };
    if !home.join("cache/version.lock").is_file() {
        return;
    }
    if LEGACY_NUDGE_SHOWN.swap(true, Ordering::Relaxed) {
        return;
    }
    let Ok(display) = sk_kernel::render_runtime_path(&home, true) else {
        return;
    };
    eprintln!(
        "Note: Run `spec-kitty migrate` to clean up legacy project files and use the global \
         runtime ({display})."
    );
}

/// Render the `section:<slug>` selector against the project's `charter.md`.
fn section(
    root: &Path,
    include: &str,
    selector: &IncludeSelector,
    json_output: bool,
) -> Result<u8, CompatError> {
    // The `--include` path keeps its prior tolerance: the strict ordinary read's
    // failure is treated as absent prose here, exactly as before this reader
    // gained an error phase for the action-context render.
    let markdown = super::read_charter_markdown(root)
        .ok()
        .flatten()
        .unwrap_or_default();
    match sk_doctrine::context_render::render_section_include(&markdown, &selector.identifier) {
        Some(text) => emit(include, &PythonString::from_scalars(&text), json_output),
        None => Err(selector_error(IncludeError::Missing {
            message: format!("No charter section found for selector '{include}'."),
        })),
    }
}

/// Print the rendered selector in the reference's text or `--json` envelope.
///
/// The envelope is the reference's own object — `result`, `success`, `include`,
/// `context`, `text`, in that insertion order — rendered by the ONE Kernel
/// `json.dumps` encoder with `indent=2, ensure_ascii=True`. That is what makes an
/// authored lone surrogate reach stdout as `\uXXXX` (exactly like the reference)
/// instead of being lost, and what keeps non-ASCII text escaped as the reference
/// escapes it.
///
/// Both arms then hand the exact text to the ONE stdout policy composition
/// ([`crate::compat::python_output`]): the startup `sys.stdout` encoding and
/// error handler decide the physical bytes, including the raw escaped byte for a
/// `\udc80..\udcff` unit and the reference's `UnicodeEncodeError` phase when the
/// policy cannot carry a unit. No partial output is written on that refusal.
fn emit(include: &str, text: &PythonString, json_output: bool) -> Result<u8, CompatError> {
    if json_output {
        let document = json_envelope(include, text);
        let document = PythonString::from_scalars(&document);
        write_source_json(&document).map_err(|error| stdout_error(error, include, true))?;
        return Ok(0);
    }
    write_source_text(text).map_err(|error| stdout_error(error, include, false))?;
    Ok(0)
}

/// The reference `--json` envelope as one rendered document.
fn json_envelope(include: &str, text: &PythonString) -> String {
    let mut envelope = PythonJsonDict::new();
    let mut member = |key: &str, value: PythonJsonValue| {
        envelope.insert(PythonString::from_scalars(key), value);
    };
    member("result", PythonJsonValue::scalar("success"));
    member("success", PythonJsonValue::Bool(true));
    member(
        "include",
        PythonJsonValue::String(PythonString::from_scalars(include)),
    );
    member("context", PythonJsonValue::String(text.clone()));
    member("text", PythonJsonValue::String(text.clone()));
    PythonJsonValue::ExactObject(envelope)
        .render_python_with(PythonJsonRenderOptions::new(Some(2)).sort_keys(false))
        .to_scalar_string()
        .expect("ensure_ascii=True renders only scalar ASCII")
}

/// Render one physical stdout failure the way the reference's phase reports it.
///
/// An `UnicodeEncodeError`/handler failure is the reference's late output-encoding
/// step: stdout stays untouched and the message names the actual codec failure,
/// the reference's own `PYTHONIOENCODING` hint, and the `--json` route that
/// carries the value. A closed pipe/descriptor keeps the reference's exit class
/// (1 for the Rich text arm, 120 for the builtin-`print` JSON arm); the native
/// stderr line is the dispatcher's ordinary `Error: <message>` shape.
fn stdout_error(error: StdoutError, include: &str, json_output: bool) -> CompatError {
    let message = error.to_string();
    if error.is_encode_failure() {
        let what = if json_output {
            "cannot print the JSON envelope of --include"
        } else {
            "cannot print the rendered text of --include"
        };
        return CompatError::domain(format!(
            "{what} '{include}': {message}\n{PYTHONIOENCODING_HINT}\nThe rendered value is \
             complete; re-run with --json, which escapes those code points exactly like the \
             reference's json.dumps."
        ));
    }
    let exit = error.exit_code();
    let envelope = serde_json::json!({
        "result": "error",
        "success": false,
        "error": message,
    });
    CompatError::envelope_with_exit(exit, envelope, message)
}

/// Turn one selector refusal into the CLI's error (a coded gap, or a reference `ValueError`).
fn selector_error(error: IncludeError) -> CompatError {
    let message = error.to_string();
    let Some(code) = error.code() else {
        return refusal(message);
    };
    let envelope = serde_json::json!({
        "result": "error",
        "success": false,
        "error": message,
        "code": code,
    });
    CompatError::result_envelope(envelope, message)
}

/// A reference-style refusal: the shared error envelope with no machine code.
fn refusal(message: String) -> CompatError {
    let envelope = serde_json::json!({
        "result": "error",
        "success": false,
        "error": message,
    });
    CompatError::result_envelope(envelope, message)
}

/// Compose one canonical-access construction failure.
///
/// The reference raises `ActiveCharterConfigError` (with its `ACTIVE_CHARTER_CONFIG_INVALID`
/// code) while the effective deny is decided; a plain construction failure carries no
/// code. Both reach the operator as the same `Error: <message>` line.
fn access_error(error: IncludeAccessError) -> CompatError {
    let message = error.message().to_owned();
    if error.is_config() {
        let envelope = serde_json::json!({
            "result": "error",
            "success": false,
            "error": message,
            "code": "ACTIVE_CHARTER_CONFIG_INVALID",
        });
        return CompatError::result_envelope(envelope, message);
    }
    refusal(message)
}
