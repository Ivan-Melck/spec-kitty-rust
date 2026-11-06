//! Canonical text rendering of the resolved `charter context` payload.
//!
//! Mirrors the text half of the reference `charter context` command:
//!
//! * `src/charter/activation/context_renderers/bootstrap_text.py`
//! * `src/charter/activation/context_renderers/section_bodies.py`
//! * `src/charter/activation/context_renderers/authority_paths.py`
//! * `src/charter/activation/context_renderers/selection_block.py`
//!   (`_extend_named_artifact_lines` / fetch stanza cadence)
//! * `src/charter/activation/context_renderers/artifact_bodies.py`
//! * `src/charter/activation/compact.py` (`render_compact_view` text)
//!   and `src/charter/activation/context_renderers/compact_governance.py`
//!
//! One formatter consumes the canonical [`CharterContext`] built by
//! [`crate::build_action_context`]; it holds no second profile, YAML,
//! activation or scope model. Nothing here decides *which* doctrine is in
//! force — the payload already carries that answer.
//!
//! Disclosure: the reference composes the same blocks from several inputs this
//! payload does not carry (the retired references manifest index, org-pack
//! provenance names, the charter compiler's catalog). Those inputs are explicit
//! [`RenderInputs`] data, and an absent input renders the reference's own
//! degraded wording instead of an invented one.

use std::path::Path;

use sk_kernel::PythonString;
use sk_kernel::python_json::{PythonJsonRenderOptions, PythonJsonValue};
use sk_kernel::python_text::{is_whitespace, is_word, lower_codepoints, regex_ignore_case_key};

use crate::context::{ArtifactEntry, CharterContext, ContextMode, Delivery, entry_text};
use crate::profile_sections::{
    ProfileSectionsError, ProfileSectionsInputs, render_profile_sections,
};
use crate::selection_block::{SelectionBlockError, SelectionBlockInputs, render_selection_block};

/// Why a charter-text render could not produce its prepared blocks.
///
/// The render owner merges the two prepared-block error families it consumes; it
/// never invents a success for a block it could not prepare.
#[derive(Debug)]
pub enum RenderError {
    /// The prepared profile-cited block.
    Profile(ProfileSectionsError),
    /// The prepared global-selection block.
    Selection(SelectionBlockError),
}

impl std::fmt::Display for RenderError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Profile(error) => write!(formatter, "{error}"),
            Self::Selection(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for RenderError {}

impl From<ProfileSectionsError> for RenderError {
    fn from(error: ProfileSectionsError) -> Self {
        Self::Profile(error)
    }
}

impl From<SelectionBlockError> for RenderError {
    fn from(error: SelectionBlockError) -> Self {
        Self::Selection(error)
    }
}

/// The bootstrap-mode header.
pub const BOOTSTRAP_HEADER: &str = "Charter Context (Bootstrap):";
/// The first-load guidance line (emitted by every bootstrap render).
pub const FIRST_LOAD_GUIDANCE: &str =
    "  - This is the first load for this action. Use the summary and follow references as needed.";
/// The policy-summary header.
pub const POLICY_SUMMARY_HEADER: &str = "Policy Summary:";
/// The reference's fallback when `charter.md` has no summary bullets.
pub const NO_POLICY_SUMMARY_MESSAGE: &str =
    "  - No explicit policy summary section found in charter.md.";
/// The reference-docs header.
pub const REFERENCE_DOCS_HEADER: &str = "Reference Docs:";
/// The reference's fixed `missing`-rail text (no charter surface at all).
///
/// `context.py`'s `_missing_charter_context_result` renders exactly these three
/// lines, with no action header, no summary and no state write.
pub const MISSING_CHARTER_TEXT: &str = "Charter Context:\n  - Charter file not found at `.kittify/charter/charter.yaml`.\n  - Run `spec-kitty charter interview` then `spec-kitty charter generate`.";
/// The reference's fallback when no reference pointer resolves.
pub const MISSING_REFERENCES_MESSAGE: &str = "  - No references manifest found.";
/// The compact view's `(none)` label.
pub const NONE_LABEL: &str = "(none)";
/// The declared-governance-reading header.
pub const GOVERNANCE_READING_HEADER: &str = "Required Governance Reading:";
/// The authority-paths header.
pub const AUTHORITY_PATHS_HEADER: &str = "Project authority paths:";
/// The fetch stanza's fallback when-doing clause.
pub const DEFAULT_WHEN_CLAUSE: &str = "are about to apply a code change";
/// The consumer-facing advice for a project without specialist guidance.
pub const CHARTER_EXTENSION_ADVISORY: &str = "No specialist guidance exists for this project's \
language; add tech-specific guidelines to your local charter (see 'Extend your charter for an \
unsupported language').";

const TERMINOLOGY_WHEN: &str = "canonical terminology — when you encounter a domain term in the \
diff, grep this directory";
const ADR_WHEN: &str = "architectural intent — when you change a structural boundary, read the \
relevant ADR (filed by era, for example docs/adr/4.x/)";
const DEFAULT_CHARTER_DECLARED_WHEN_CLAUSE: &str =
    "consult when you change content under this directory";

/// The two default authority directories, with their reference when-doing copy.
///
/// Each home keeps a trailing slash so a reader can grep `docs/context/`
/// unambiguously; rendering stays existence-gated.
pub const DEFAULT_AUTHORITY_PATHS: [(&str, &str); 2] =
    [("docs/context/", TERMINOLOGY_WHEN), ("docs/adr/", ADR_WHEN)];

/// The charter section shown for each action, in render order.
///
/// Mirrors `ACTION_CRITICAL_SECTIONS`; an action with no entry renders no
/// section block.
pub fn action_critical_sections(action: &str) -> &'static [&'static str] {
    match action {
        "implement" | "review" => &[
            "Terminology Canon",
            "Code Review Checklist",
            "Regression Vigilance",
        ],
        _ => &[],
    }
}

/// The when-doing clause for a critical section (falls back to the default).
pub fn critical_section_when_clause(heading: &str) -> &'static str {
    match heading {
        "Terminology Canon" => "rename or introduce a term in the diff",
        "Code Review Checklist" => "prepare a WP for review",
        "Regression Vigilance" => "perform a terminology cutover",
        _ => DEFAULT_WHEN_CLAUSE,
    }
}

/// Exact-codepoint when-clause lookup for one authored critical heading.
///
/// Source keeps these in a `dict` keyed by the heading string, so equality is
/// exact code-point equality; a heading that differs in any unit (a non-breaking
/// space, a combining mark) takes the module default.
pub fn critical_section_when_clause_exact(heading: &PythonString) -> PythonString {
    for (name, clause) in [
        (
            "Terminology Canon",
            "rename or introduce a term in the diff",
        ),
        ("Code Review Checklist", "prepare a WP for review"),
        ("Regression Vigilance", "perform a terminology cutover"),
    ] {
        if heading == &PythonString::from_scalars(name) {
            return PythonString::from_scalars(clause);
        }
    }
    PythonString::from_scalars(DEFAULT_WHEN_CLAUSE)
}

/// The `section:<slug>` fetch selector for a heading (finite scalar adapter).
pub fn critical_section_selector(heading: &str) -> String {
    critical_section_selector_exact(&PythonString::from_scalars(heading))
        .to_scalar_string()
        .expect("an ASCII section selector")
}

/// Exact-codepoint `section:<slug>` selector for one authored heading.
///
/// The ONE selector authority: Source `critical_section_selector` derives the
/// slug from `heading.lower()` and `[^a-z0-9]+`, so a heading with a non-ASCII
/// letter that Python lowercases to ASCII keeps that letter. Authored code
/// points are never narrowed here.
pub fn critical_section_selector_exact(heading: &PythonString) -> PythonString {
    let mut selector = PythonString::from_scalars("section:");
    selector.append(&slugify_heading_exact(heading));
    selector
}

/// Lowercase a heading into its contract slug (finite scalar adapter).
pub fn slugify_heading(heading: &str) -> String {
    slugify_heading_exact(&PythonString::from_scalars(heading))
        .to_scalar_string()
        .expect("an ASCII heading slug")
}

/// Exact-codepoint heading slug: Python `str.lower()`, then non-`[a-z0-9]` runs
/// collapse to one hyphen and leading/trailing hyphens are stripped.
pub fn slugify_heading_exact(heading: &PythonString) -> PythonString {
    let mut units: Vec<u32> = Vec::new();
    let mut pending_hyphen = false;
    for code in lower_codepoints(heading.codepoints()) {
        if code < 0x80 && u8::try_from(code).is_ok_and(|byte| byte.is_ascii_alphanumeric()) {
            if pending_hyphen && !units.is_empty() {
                units.push(u32::from(b'-'));
            }
            pending_hyphen = false;
            units.push(code);
        } else {
            pending_hyphen = true;
        }
    }
    PythonString::from_units(units).expect("slug units are valid code points")
}

pub use crate::reference_pointers::SelectedReference as ReferenceDoc;

/// The governance summary the compact rail prints.
///
/// The caller resolves it (the reference's `_resolve_governance_summary`);
/// the renderer only formats. `None` renders the reference's degraded
/// `(none)` labels plus the caller's diagnostic.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CompactGovernance {
    /// The resolved template set, or `None` for the `(none)` label.
    pub template_set: Option<String>,
    /// The resolved paradigms (empty renders `(none)`).
    pub paradigms: Vec<String>,
    /// The resolved tools (empty renders `(none)`).
    pub tools: Vec<String>,
    /// The resolver diagnostics, printed as one `Diagnostics:` line.
    pub diagnostics: Vec<String>,
    /// The resolver's directive canon, merged into the `Directive IDs:` block.
    pub resolver_directive_ids: Vec<String>,
}

/// The optional inputs a text render needs beyond the resolved payload.
#[derive(Clone, Debug, Default)]
pub struct RenderInputs<'a> {
    /// The project `charter.md` prose (display-only; sections and summary).
    pub charter_markdown: Option<&'a str>,
    /// The repository root, which existence-gates the authority paths.
    pub project_root: Option<&'a Path>,
    /// Charter-declared authority paths, appended after the defaults.
    pub declared_authority_paths: &'a [String],
    /// The `catalog.references` pointers that resolved to a readable document.
    pub reference_docs: &'a [ReferenceDoc],
    /// The compact rail's governance summary.
    pub compact_governance: Option<&'a CompactGovernance>,
    /// The project's stored/inferred language list (compact footnote).
    pub languages: Option<&'a [String]>,
    /// Whether the language authority says this project lacks specialist
    /// guidance (the compact advisory line).
    pub lacks_specialist_guidance: bool,
    /// The resolved profile and the canonical repositories the prepared
    /// profile-cited block reads. `None` renders no profile block (the
    /// reference's `profile=None`), never a fabricated empty section.
    pub profile_sections: Option<ProfileSectionsInputs<'a>>,
    /// The effective charter selection and the canonical repositories the
    /// prepared global-selection block reads. `None` renders no selection block
    /// (the reference's `charter_config=None`), never a fabricated section.
    pub selection_block: Option<SelectionBlockInputs<'a>>,
}

/// Render the payload in its own [`ContextMode`] with no extra inputs.
///
/// # Errors
///
/// A prepared profile-section body this crate cannot project.
pub fn render(context: &CharterContext) -> Result<String, RenderError> {
    render_with(context, &RenderInputs::default())
}

/// Render the payload in its own [`ContextMode`].
///
/// # Errors
///
/// A prepared profile-section body this crate cannot project; the finite scalar
/// adapter also refuses non-scalar rendered text via the exact entry point.
pub fn render_with(
    context: &CharterContext,
    inputs: &RenderInputs<'_>,
) -> Result<String, RenderError> {
    Ok(render_with_exact(context, inputs)?
        .to_scalar_string()
        .expect("finite render adapter: use render_with_exact for Python strings"))
}

/// Render the payload without narrowing Python text to Rust scalar strings.
///
/// # Errors
///
/// A prepared profile-section body this crate cannot project.
pub fn render_with_exact(
    context: &CharterContext,
    inputs: &RenderInputs<'_>,
) -> Result<PythonString, RenderError> {
    match context.mode {
        ContextMode::Bootstrap => render_bootstrap_exact(context, inputs),
        ContextMode::Compact => render_compact_exact(context, inputs),
        // The `missing` rail's data is fixed: the reference reads no prose, no
        // catalogue and no governance block for it, so this arm deliberately
        // consults none of `inputs`.
        ContextMode::Missing => Ok(PythonString::from_scalars(MISSING_CHARTER_TEXT)),
    }
}

/// Render bootstrap text through the finite scalar adapter.
///
/// # Errors
///
/// A prepared profile-section body this crate cannot project, or non-scalar text.
pub fn render_bootstrap(
    context: &CharterContext,
    inputs: &RenderInputs<'_>,
) -> Result<String, RenderError> {
    Ok(render_bootstrap_exact(context, inputs)?
        .to_scalar_string()
        .expect("finite render adapter: use render_bootstrap_exact for Python strings"))
}

/// The one bootstrap text formatter, carrying exact authored row text.
///
/// # Errors
///
/// A prepared profile-section body this crate cannot project.
pub fn render_bootstrap_exact(
    context: &CharterContext,
    inputs: &RenderInputs<'_>,
) -> Result<PythonString, RenderError> {
    // WP05 (NFR-001): the action-critical block is rendered ONCE here and the
    // same exact block is handed to the budget boundary below. Re-rendering it
    // from a second parser (or recovering it by splitting the finished text)
    // would let the two readings disagree; the render owner passes its own
    // result instead.
    let section_block = render_critical_section_bodies(
        inputs.charter_markdown.unwrap_or_default(),
        &context.action,
    );
    // CF73: the profile-cited block is prepared EXACTLY ONCE here (the one
    // profile-sections owner), pushed into the text and handed to the same
    // budget boundary. `None` is the reference's `profile=None`; a resolved
    // profile with no attested section renders the reference's empty block.
    let prepared_profile = match inputs.profile_sections.as_ref() {
        Some(profile) => render_profile_sections(profile)?,
        None => PythonString::default(),
    };
    // CF75: the global-selection block is prepared EXACTLY ONCE here (the one
    // selection-block owner), inserted at the reference's Source position —
    // after the profile block and before the Action Doctrine heading — and the
    // SAME block is handed to the budget boundary below.
    let prepared_selection = match inputs.selection_block.as_ref() {
        Some(selection) => render_selection_block(selection)?,
        None => PythonString::default(),
    };
    let mut lines: Vec<String> = vec![
        BOOTSTRAP_HEADER.to_owned(),
        format!("  - Source: {}", source_label(context, inputs)),
        FIRST_LOAD_GUIDANCE.to_owned(),
        String::new(),
        POLICY_SUMMARY_HEADER.to_owned(),
    ];
    append_policy_summary(&mut lines, inputs.charter_markdown);
    push_block(&mut lines, authority_block(inputs));
    push_block(&mut lines, governance_reading_block(context));
    push_block(&mut lines, section_block.clone());
    let mut exact_lines: Vec<PythonString> = lines
        .iter()
        .map(|line| PythonString::from_scalars(line))
        .collect();
    if !prepared_profile.is_empty() {
        exact_lines.push(PythonString::default());
        exact_lines.push(prepared_profile.clone());
    }
    if !prepared_selection.is_empty() {
        exact_lines.push(PythonString::default());
        exact_lines.push(prepared_selection.clone());
    }
    exact_lines.push(PythonString::default());
    exact_lines.push(PythonString::from_scalars(&format!(
        "Action Doctrine ({}):",
        context.action
    )));
    append_entry_lines(&mut exact_lines, "Directives", &context.directives);
    append_entry_lines(&mut exact_lines, "Tactics", &context.tactics);
    append_entry_lines(&mut exact_lines, "Styleguides", &context.styleguides);
    append_entry_lines(&mut exact_lines, "Toolguides", &context.toolguides);
    append_entry_lines(&mut exact_lines, "Procedures", &context.procedures);
    let mut references: Vec<PythonString> = vec![
        PythonString::default(),
        PythonString::from_scalars(REFERENCE_DOCS_HEADER),
    ];
    append_reference_lines(&mut references, inputs.reference_docs);
    exact_lines.extend(references);
    let text = joined(&exact_lines, "\n");
    let prepared = PreparedCharterBlocks {
        critical_sections: PythonString::from_scalars(&section_block),
        profile_sections: prepared_profile,
        selection_block: prepared_selection,
    };
    Ok(apply_prepared_budget(text, &context.action, &prepared))
}

/// The exact blocks the bootstrap budget boundary consumes.
///
/// Every field is produced by the block's own render owner (never re-read from
/// the finished text). The budget engine's remaining Source inputs — the
/// profile-cited and global-selection blocks — have no Native renderer yet, so
/// the boundary passes Source's own empty-block value for them instead of a
/// fabricated section (see the CF71 report's disclosed seam).
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PreparedCharterBlocks {
    /// The rendered `Action-Critical Charter Sections (<action>):` block.
    pub critical_sections: PythonString,
    /// The rendered `Profile-Cited …` / `Profile-Resolved …` /
    /// `Profile-Suggested …` block, prepared exactly once by the one
    /// profile-sections owner. Empty is the reference's `None`/no-section case.
    pub profile_sections: PythonString,
    /// The rendered combined eight-kind `Selected …` block, prepared exactly
    /// once by the one selection-block owner in the reference's Source order.
    /// Empty is the reference's no-selection case.
    pub selection_block: PythonString,
}

/// Apply Source `_enforce_token_budget` to a fully rendered bootstrap text.
///
/// The budget runs where Source runs it — after the whole text, including the
/// reference rows, is rendered and before the caller's first-load mark. It never
/// re-renders a block: the prepared critical block is the one already in `text`.
fn apply_prepared_budget(
    text: PythonString,
    action: &str,
    prepared: &PreparedCharterBlocks,
) -> PythonString {
    crate::text_budget::enforce_token_budget(
        &text,
        &PythonString::from_scalars(action),
        &prepared.profile_sections,
        &prepared.critical_sections,
        &prepared.selection_block,
        crate::text_budget::BUDGET_DEFAULT,
        None,
    )
}

/// Render the compact steady-state governance block.
///
/// Carries every delivered kind's ids and the charter section anchors; only
/// long-form prose is elided (the reference's FR-034 contract).
///
/// # Errors
///
/// A prepared profile-section body this crate cannot project, or non-scalar text.
pub fn render_compact(
    context: &CharterContext,
    inputs: &RenderInputs<'_>,
) -> Result<String, RenderError> {
    Ok(render_compact_exact(context, inputs)?
        .to_scalar_string()
        .expect("finite compact adapter: use render_compact_exact for Python strings"))
}

/// Render the compact steady-state governance block, including the prepared
/// profile augmentation and the shared character budget.
///
/// The reference appends non-empty augmented blocks (authority, governance
/// references, action-critical sections, profile) and only then applies the
/// shared budget. Native's compact rail renders only the compact governance body
/// today, so this owner appends the profile block when it is non-empty and then
/// applies the SAME budget owner — never a universal compact cap, and never a
/// fabricated augmentation when the block is empty (the reference's
/// `if not augmented_blocks: return text`).
///
/// # Errors
///
/// A prepared profile-section body this crate cannot project.
pub fn render_compact_exact(
    context: &CharterContext,
    inputs: &RenderInputs<'_>,
) -> Result<PythonString, RenderError> {
    let text = PythonString::from_scalars(&render_compact_body(context, inputs));
    let prepared_profile = match inputs.profile_sections.as_ref() {
        Some(profile) => render_profile_sections(profile)?,
        None => PythonString::default(),
    };
    if prepared_profile.is_empty() {
        return Ok(text);
    }
    let mut combined = text;
    combined.push_scalars("\n\n");
    combined.append(&prepared_profile);
    Ok(crate::text_budget::enforce_token_budget(
        &combined,
        &PythonString::from_scalars(&context.action),
        &prepared_profile,
        &PythonString::default(),
        &PythonString::default(),
        crate::text_budget::BUDGET_DEFAULT,
        None,
    ))
}

/// The compact body without any augmentation (the pre-CF73 render).
fn render_compact_body(context: &CharterContext, inputs: &RenderInputs<'_>) -> String {
    let governance = inputs.compact_governance;
    let rendered = |values: &[String]| {
        if values.is_empty() {
            NONE_LABEL.to_owned()
        } else {
            values.join(", ")
        }
    };
    let template_set = governance
        .and_then(|summary| summary.template_set.clone())
        .unwrap_or_else(|| NONE_LABEL.to_owned());
    let empty: Vec<String> = Vec::new();
    let paradigms = governance.map_or(&empty, |summary| &summary.paradigms);
    let tools = governance.map_or(&empty, |summary| &summary.tools);

    let mut lines: Vec<String> = vec![
        "Governance:".to_owned(),
        format!("  - Template set: {template_set}"),
        format!("  - Paradigms: {}", rendered(paradigms)),
        format!("  - Tools: {}", rendered(tools)),
    ];

    let mut directive_ids: Vec<String> = context
        .all_directives
        .iter()
        .map(|directive| directive.id.clone())
        .collect();
    if let Some(summary) = governance {
        directive_ids.extend(summary.resolver_directive_ids.iter().cloned());
    }
    dedupe(&mut directive_ids);
    append_section(&mut lines, "Directive IDs:", &directive_ids);
    append_section(&mut lines, "Tactic IDs:", &entry_ids(&context.tactics));
    append_section_if_present(
        &mut lines,
        "Styleguide IDs:",
        &entry_ids(&context.styleguides),
    );
    append_section_if_present(
        &mut lines,
        "Toolguide IDs:",
        &entry_ids(&context.toolguides),
    );
    append_section_if_present(
        &mut lines,
        "Procedure IDs:",
        &entry_ids(&context.procedures),
    );
    append_section(
        &mut lines,
        "Section Anchors:",
        &section_anchors(inputs.charter_markdown),
    );
    if let Some(summary) = governance
        && !summary.diagnostics.is_empty()
    {
        lines.push(format!(
            "  - Diagnostics: {}",
            summary.diagnostics.join(" | ")
        ));
    }
    if let Some(languages) = inputs.languages
        && !languages.is_empty()
    {
        let mut sorted: Vec<&str> = languages.iter().map(String::as_str).collect();
        sorted.sort_unstable();
        lines.push(format!("  - Languages: {}", sorted.join(", ")));
        if inputs.lacks_specialist_guidance {
            lines.push(format!("  - Advisory: {CHARTER_EXTENSION_ADVISORY}"));
        }
    }
    lines.join("\n")
}

/// Render one `section:<slug>` include selector against `charter.md`.
///
/// A recognized critical-section slug never dead-ends: when the heading is
/// absent it renders the reference's honest placeholder pointing at the
/// authoring surface. `None` means the selector does not address a known
/// section (the caller reports an unknown selector).
pub fn render_section_include(charter_markdown: &str, selector: &str) -> Option<String> {
    let cleaned = selector.trim();
    if cleaned.is_empty() {
        return None;
    }
    let headings = [
        "Terminology Canon",
        "Code Review Checklist",
        "Regression Vigilance",
    ];
    for heading in headings {
        if slugify_heading(heading) != cleaned {
            continue;
        }
        return Some(match extract_section_body(charter_markdown, heading) {
            Some(body) if !body.is_empty() => format!("### {heading}\n{body}"),
            Some(_) => format!("### {heading}"),
            None => format!(
                "_This charter has not yet authored a **{heading}** section. \
                 Add one to `.kittify/charter/charter.md`._"
            ),
        });
    }
    None
}

/// Render the `Action-Critical Charter Sections (<action>)` block.
///
/// Missing headings emit the fetch + when-doing stanza so the advertised
/// `section:<slug>` selector still has a recovery rail.
pub fn render_critical_section_bodies(charter_markdown: &str, action: &str) -> String {
    let headings = action_critical_sections(action);
    if headings.is_empty() {
        return String::new();
    }
    let mut blocks: Vec<String> = vec![format!("Action-Critical Charter Sections ({action}):")];
    for heading in headings {
        blocks.push(String::new());
        blocks.push(format!("### {heading}"));
        if let Some(body) = extract_section_body(charter_markdown, heading)
            && !body.is_empty()
        {
            blocks.push(body);
        }
        blocks.extend(fetch_stanza_lines(
            &critical_section_selector(heading),
            critical_section_when_clause(heading),
            "  ",
        ));
    }
    blocks.join("\n")
}

/// The two-line fetch + when-doing stanza, at `indent` (finite scalar adapter).
///
/// This is the checked scalar view of [`fetch_stanza_lines_exact`]: every
/// authored value is first represented exactly and then projected back, so a
/// caller with scalar data keeps byte-identical output and the exact API stays
/// the ONE composition authority.
pub fn fetch_stanza_lines(selector: &str, when_clause: &str, indent: &str) -> Vec<String> {
    fetch_stanza_lines_exact(
        &PythonString::from_scalars(selector),
        &PythonString::from_scalars(when_clause),
        &PythonString::from_scalars(indent),
        None,
    )
    .into_iter()
    .map(|line| {
        line.to_scalar_string()
            .expect("a scalar fetch stanza carries scalar lines")
    })
    .collect()
}

/// The admitted fetch arguments Source projects from an ownership fact.
///
/// Read-only rendering data: it grants no authority. The reference revalidates
/// both values through its ownership minter before reading charter context; a
/// caller with no admitted fact passes `None` and the stanza carries no flags.
#[derive(Clone, Copy, Debug)]
pub struct FetchScope<'a> {
    /// The admitted owned checkout root, spelled verbatim.
    pub owned_root: &'a PythonString,
    /// The admitted canonical Mission handle, spelled verbatim.
    pub mission_handle: &'a PythonString,
}

/// The canonical fetch command for `selector` (`fetch_stanza.fetch_command`).
///
/// One composition owner: argv order, `shlex.join` quoting (CPython 3.11.15
/// `shlex.quote`: ASCII word characters plus `@%+=:,./-` stay bare; everything
/// else is single-quoted with the `'"'"'` escape) and the optional scoped
/// `--owned-checkout`/`--mission` pair.
pub fn fetch_command(selector: &PythonString, scope: Option<&FetchScope<'_>>) -> PythonString {
    let mut argv = vec![
        PythonString::from_scalars("spec-kitty"),
        PythonString::from_scalars("charter"),
        PythonString::from_scalars("context"),
        PythonString::from_scalars("--include"),
        selector.clone(),
    ];
    if let Some(scope) = scope {
        argv.push(PythonString::from_scalars("--owned-checkout"));
        argv.push(scope.owned_root.clone());
        argv.push(PythonString::from_scalars("--mission"));
        argv.push(scope.mission_handle.clone());
    }
    shlex_join(&argv)
}

/// CPython `shlex.join` over exact code points.
fn shlex_join(argv: &[PythonString]) -> PythonString {
    let mut joined = PythonString::default();
    for (position, word) in argv.iter().enumerate() {
        if position > 0 {
            joined.push_scalars(" ");
        }
        joined.append(&shlex_quote(word));
    }
    joined
}

/// CPython 3.11.15 `shlex.quote` over exact code points.
fn shlex_quote(word: &PythonString) -> PythonString {
    if word.is_empty() {
        return PythonString::from_scalars("''");
    }
    let safe = word.codepoints().iter().all(|&code| {
        code < 0x80
            && u8::try_from(code)
                .is_ok_and(|byte| byte.is_ascii_alphanumeric() || b"_@%+=:,./-".contains(&byte))
    });
    if safe {
        return word.clone();
    }
    let mut quoted = PythonString::from_scalars("'");
    for &code in word.codepoints() {
        if code == u32::from(b'\'') {
            quoted.push_scalars("'\"'\"'");
        } else {
            quoted
                .push_codepoint(code)
                .expect("an existing code point stays valid");
        }
    }
    quoted.push_scalars("'");
    quoted
}

/// The exact two-line fetch + when-doing stanza, at `indent`.
pub fn fetch_stanza_lines_exact(
    selector: &PythonString,
    when_clause: &PythonString,
    indent: &PythonString,
    scope: Option<&FetchScope<'_>>,
) -> Vec<PythonString> {
    let clause = exact_strip(when_clause);
    let clause = if clause.is_empty() {
        PythonString::from_scalars(DEFAULT_WHEN_CLAUSE)
    } else {
        clause
    };
    let clause = normalize_when_clause_exact(&clause);
    let mut run = indent.clone();
    run.push_scalars("Run: ");
    run.append(&fetch_command(selector, scope));
    let mut when = indent.clone();
    when.push_scalars("When you ");
    when.append(&clause);
    when.push_scalars(", run this command and apply the returned rule.");
    vec![run, when]
}

/// Map an authored when-clause into the contract's closed lead-in set (scalar).
///
/// Checked finite view of [`normalize_when_clause_exact`] for the module's scalar
/// unit tests; production composition goes through the exact function.
#[cfg(test)]
fn normalize_when_clause(clause: &str) -> String {
    normalize_when_clause_exact(&PythonString::from_scalars(clause))
        .to_scalar_string()
        .expect("a scalar when clause")
}

/// Exact Source `_normalize_when_clause` over authored code points.
fn normalize_when_clause_exact(clause: &PythonString) -> PythonString {
    let stripped = exact_strip(clause);
    if stripped.is_empty() {
        return PythonString::from_scalars(DEFAULT_WHEN_CLAUSE);
    }
    let candidate = without_leading_when(stripped.codepoints());
    if match_lead_in(&candidate) {
        return PythonString::from_units(candidate).expect("a slice of valid code points");
    }
    if gerund_lead(&candidate) {
        return PythonString::from_scalars(DEFAULT_WHEN_CLAUSE);
    }
    let without_period = match candidate.split_last() {
        Some((&0x2e, rest)) => rest.to_vec(),
        _ => candidate,
    };
    if without_period.is_empty() {
        return PythonString::from_scalars(DEFAULT_WHEN_CLAUSE);
    }
    // `without_period[0].lower() + without_period[1:]`: only the first code
    // point is lowered, and Python lower may expand it (U+0130).
    let lowered_first = lower_codepoints(&without_period[..1]);
    let mut anchored = PythonString::from_units(lowered_first).expect("lowered units");
    anchored.append(
        &PythonString::from_units(without_period[1..].to_vec()).expect("a code-point slice"),
    );
    let mut result = PythonString::from_scalars("need to ");
    result.append(&anchored);
    result
}

/// `^when\s+` (case-insensitive), removed once like Source's `sub(count=1)`.
fn without_leading_when(candidate: &[u32]) -> Vec<u32> {
    let mut index = 0usize;
    for expected in "when".chars() {
        match candidate.get(index) {
            Some(&actual)
                if regex_ignore_case_key(actual) == regex_ignore_case_key(u32::from(expected)) =>
            {
                index += 1;
            }
            _ => return candidate.to_vec(),
        }
    }
    let run_start = index;
    while index < candidate.len() && is_whitespace(candidate[index]) {
        index += 1;
    }
    if index == run_start {
        return candidate.to_vec();
    }
    candidate[index..].to_vec()
}

/// Source `_LEAD_IN_RE`: the ordered six-lead-in alternation with `\b`.
fn match_lead_in(candidate: &[u32]) -> bool {
    const PATTERNS: [&[&str]; 6] = [
        &["are", "about", "to"],
        &["need", "to"],
        &["encounter"],
        &["introduce"],
        &["rename"],
        &["review"],
    ];
    PATTERNS
        .iter()
        .any(|words| match_lead_words(candidate, words))
}

/// One literal-word lead-in: `\s+` between words and `\b` after the last.
fn match_lead_words(candidate: &[u32], words: &[&str]) -> bool {
    let mut index = 0usize;
    for (position, word) in words.iter().enumerate() {
        if position > 0 {
            let run_start = index;
            while index < candidate.len() && is_whitespace(candidate[index]) {
                index += 1;
            }
            if index == run_start {
                return false;
            }
        }
        for expected in word.chars() {
            let Some(&actual) = candidate.get(index) else {
                return false;
            };
            if regex_ignore_case_key(actual) != regex_ignore_case_key(u32::from(expected)) {
                return false;
            }
            index += 1;
        }
    }
    candidate.get(index).is_none_or(|&next| !is_word(next))
}

/// Source `_GERUND_LEAD_RE`: `^[A-Za-z]+ing\b` (lowercase `ing`).
fn gerund_lead(candidate: &[u32]) -> bool {
    let mut run = 0usize;
    while run < candidate.len()
        && candidate[run] < 0x80
        && u8::try_from(candidate[run]).is_ok_and(|byte| byte.is_ascii_alphabetic())
    {
        run += 1;
    }
    if run < 4 || candidate[run - 3..run] != [u32::from(b'i'), u32::from(b'n'), u32::from(b'g')] {
        return false;
    }
    candidate.get(run).is_none_or(|&next| !is_word(next))
}

fn source_label(context: &CharterContext, inputs: &RenderInputs<'_>) -> String {
    if let Some(root) = inputs.project_root {
        return root.join(sk_charter::CHARTER_MD).display().to_string();
    }
    if context.project_charter.charter_md_present {
        context.project_charter.charter_md_path.clone()
    } else {
        context.project_charter.path.clone()
    }
}

fn append_policy_summary(lines: &mut Vec<String>, charter_markdown: Option<&str>) {
    let summary = charter_markdown
        .map(extract_policy_summary)
        .unwrap_or_default();
    if summary.is_empty() {
        lines.push(NO_POLICY_SUMMARY_MESSAGE.to_owned());
        return;
    }
    for item in summary.iter().take(8) {
        lines.push(format!("  - {item}"));
    }
}

/// The reference's `_extract_policy_summary`: the `## Policy Summary` bullets,
/// or the first eight document bullets when no such section exists.
pub fn extract_policy_summary(content: &str) -> Vec<String> {
    let lines: Vec<&str> = content.lines().collect();
    let start = lines
        .iter()
        .position(|line| line.trim() == "## Policy Summary");
    let Some(start) = start else {
        return lines
            .iter()
            .filter(|line| line.trim_start().starts_with('-'))
            .map(|line| line.trim().trim_start_matches('-').trim().to_owned())
            .filter(|item| !item.is_empty())
            .take(8)
            .collect();
    };
    let mut summary = Vec::new();
    for line in &lines[start + 1..] {
        let stripped = line.trim();
        if stripped.starts_with("## ") {
            break;
        }
        if stripped.starts_with('-') {
            summary.push(stripped.trim_start_matches('-').trim().to_owned());
        }
    }
    summary
}

fn authority_block(inputs: &RenderInputs<'_>) -> String {
    let Some(root) = inputs.project_root else {
        return String::new();
    };
    let mut entries: Vec<(String, &str)> = Vec::new();
    for (path, when) in DEFAULT_AUTHORITY_PATHS {
        if root.join(path).is_dir() {
            entries.push((path.to_owned(), when));
        }
    }
    for declared in inputs.declared_authority_paths {
        let cleaned = declared.trim();
        if cleaned.is_empty() {
            continue;
        }
        let path = if cleaned.ends_with('/') {
            cleaned.to_owned()
        } else {
            format!("{cleaned}/")
        };
        if entries.iter().any(|(existing, _)| *existing == path) {
            continue;
        }
        if root.join(&path).is_dir() {
            entries.push((path, DEFAULT_CHARTER_DECLARED_WHEN_CLAUSE));
        }
    }
    if entries.is_empty() {
        return String::new();
    }
    let mut lines = vec![AUTHORITY_PATHS_HEADER.to_owned()];
    for (path, when) in entries {
        lines.push(format!("  - {path}: {when}"));
    }
    lines.join("\n")
}

fn governance_reading_block(context: &CharterContext) -> String {
    if context.governance_references.is_empty() {
        return String::new();
    }
    let mut lines = vec![GOVERNANCE_READING_HEADER.to_owned()];
    for status in &context.governance_references {
        match &status.warning {
            Some(warning) => lines.push(format!("  - WARNING: {warning}")),
            None => lines.push(format!("  - {}", status.path)),
        }
    }
    lines.join("\n")
}

/// Source `_append_reference_docs_lines`: one exact line per selected pointer.
///
/// The Source shape has no empty-title special case, so a missing title renders
/// the double space of `f"  - {id}: {title} ({path})"`. The selector owns the
/// cap (default 12); this formatter never truncates and never re-projects the
/// authored text.
fn append_reference_lines(lines: &mut Vec<PythonString>, reference_docs: &[ReferenceDoc]) {
    if reference_docs.is_empty() {
        lines.push(PythonString::from_scalars(MISSING_REFERENCES_MESSAGE));
        return;
    }
    for reference in reference_docs {
        let mut line = PythonString::from_scalars("  - ");
        line.append(&reference.entry.id);
        line.push_scalars(": ");
        line.append(&reference.entry.title);
        line.push_scalars(" (");
        line.append(&reference.source_text);
        line.push_scalars(")");
        lines.push(line);
    }
}

fn append_entry_lines(lines: &mut Vec<PythonString>, heading: &str, entries: &[ArtifactEntry]) {
    if entries.is_empty() {
        return;
    }
    lines.push(PythonString::from_scalars(&format!("  {heading}:")));
    for entry in entries {
        // Source rows contain the merged title and optional summary, not an
        // inline body. Keep exact code points until the physical stdout encoder.
        let mut row = PythonString::from_scalars(&format!("    - {}: ", entry.id));
        if let Some(title) = entry_text(entry.title.as_ref()) {
            row.append(title);
        } else {
            row.push_scalars(&entry.id);
        }
        if entry.delivery == Delivery::Inline
            && let Some(summary) = entry_text(entry.summary.as_ref())
        {
            row.push_scalars(" — ");
            row.append(summary);
        }
        lines.push(row);
        if entry.delivery == Delivery::Link {
            lines.extend(
                fetch_stanza_lines(
                    &format!("{}:{}", kind_token(entry.kind.as_str()), entry.id),
                    entry.when.as_deref().unwrap_or(DEFAULT_WHEN_CLAUSE),
                    "    ",
                )
                .iter()
                .map(|line| PythonString::from_scalars(line)),
            );
        }
    }
}

fn kind_token(kind: &str) -> &str {
    match kind {
        "agent_profile" => "agent-profile",
        "mission_step_contract" => "mission-step-contract",
        "glossary_pack" => "glossary-pack",
        "mission_type" => "mission-type",
        other => other,
    }
}

fn entry_ids(entries: &[ArtifactEntry]) -> Vec<String> {
    entries.iter().map(|entry| entry.id.clone()).collect()
}

fn dedupe(values: &mut Vec<String>) {
    let mut seen = std::collections::BTreeSet::new();
    values.retain(|value| seen.insert(value.clone()));
}

fn append_section(lines: &mut Vec<String>, title: &str, values: &[String]) {
    lines.push(title.to_owned());
    if values.is_empty() {
        lines.push(format!("  - {NONE_LABEL}"));
        return;
    }
    for value in values {
        lines.push(format!("  - {value}"));
    }
}

fn append_section_if_present(lines: &mut Vec<String>, title: &str, values: &[String]) {
    if !values.is_empty() {
        append_section(lines, title, values);
    }
}

/// The reference's ATX section anchors: ordered, de-duplicated heading text.
pub fn section_anchors(charter_markdown: Option<&str>) -> Vec<String> {
    let Some(content) = charter_markdown else {
        return Vec::new();
    };
    let mut seen = std::collections::BTreeSet::new();
    let mut anchors = Vec::new();
    for line in content.lines() {
        let Some(anchor) = markdown_heading(line) else {
            continue;
        };
        if seen.insert(anchor.clone()) {
            anchors.push(anchor);
        }
    }
    anchors
}

fn markdown_heading(line: &str) -> Option<String> {
    let stripped = line.trim();
    if !stripped.starts_with('#') {
        return None;
    }
    let level = stripped
        .chars()
        .take_while(|character| *character == '#')
        .count();
    if level == 0 || level > 6 {
        return None;
    }
    let rest = &stripped[level..];
    let anchor = rest.strip_prefix(' ')?.trim();
    if anchor.is_empty() {
        None
    } else {
        Some(anchor.to_owned())
    }
}

/// The body of `## <heading>` (a heading at any level 2..=6), or `None`.
///
/// Everything after the heading line up to the next heading at the same or a
/// higher level outside fenced code blocks is returned, with surrounding
/// newlines trimmed — the reference's slice.
pub fn extract_section_body(charter_markdown: &str, heading: &str) -> Option<String> {
    let heading = heading.trim();
    let lines: Vec<&str> = charter_markdown.split_inclusive('\n').collect();
    let mut fence: Option<(char, usize)> = None;
    let mut found: Option<(usize, usize)> = None;
    let mut offset = 0usize;
    for line in &lines {
        if let Some((marker, length)) = fence {
            if closes_fence(line, marker, length) {
                fence = None;
            }
            offset += line.len();
            continue;
        }
        if found.is_none()
            && let Some(level) = heading_match(line, heading)
        {
            found = Some((offset + line.trim_end_matches(['\n', '\r']).len(), level));
            offset += line.len();
            continue;
        }
        if let Some((marker, length)) = opens_fence(line) {
            fence = Some((marker, length));
        }
        offset += line.len();
    }
    let (body_start, heading_level) = found?;
    let remainder = &charter_markdown[body_start..];
    let end = next_section_offset(remainder, heading_level)
        .map(|value| body_start + value)
        .unwrap_or(charter_markdown.len());
    Some(
        charter_markdown[body_start..end]
            .trim_matches('\n')
            .trim_end()
            .to_owned(),
    )
}

fn heading_match(line: &str, heading: &str) -> Option<usize> {
    let trimmed = line.trim_end_matches(['\n', '\r']);
    let hashes = trimmed
        .chars()
        .take_while(|character| *character == '#')
        .count();
    if !(2..=6).contains(&hashes) {
        return None;
    }
    let rest = &trimmed[hashes..];
    let rest = rest.strip_prefix(' ')?;
    if !rest.starts_with(heading) {
        return None;
    }
    // `\b`: the heading text must not continue into another word.
    if let Some(next) = rest[heading.len()..].chars().next()
        && (next.is_alphanumeric() || next == '_')
    {
        return None;
    }
    Some(hashes)
}

fn next_section_offset(remainder: &str, heading_level: usize) -> Option<usize> {
    let mut fence: Option<(char, usize)> = None;
    let mut offset = 0usize;
    for line in remainder.split_inclusive('\n') {
        if let Some((marker, length)) = fence {
            if closes_fence(line, marker, length) {
                fence = None;
            }
            offset += line.len();
            continue;
        }
        let trimmed = line.trim_end_matches(['\n', '\r']);
        let hashes = trimmed
            .chars()
            .take_while(|character| *character == '#')
            .count();
        if (1..=6).contains(&hashes)
            && hashes <= heading_level
            && trimmed[hashes..].starts_with(' ')
        {
            return Some(offset);
        }
        if let Some((marker, length)) = opens_fence(line) {
            fence = Some((marker, length));
        }
        offset += line.len();
    }
    None
}

fn opens_fence(line: &str) -> Option<(char, usize)> {
    let trimmed = line.trim_start_matches(' ');
    if line.len() - trimmed.len() > 3 {
        return None;
    }
    let marker = trimmed.chars().next()?;
    if marker != '`' && marker != '~' {
        return None;
    }
    let length = trimmed
        .chars()
        .take_while(|character| *character == marker)
        .count();
    if length < 3 {
        return None;
    }
    Some((marker, length))
}

fn closes_fence(line: &str, marker: char, length: usize) -> bool {
    let trimmed = line.trim();
    if !trimmed.starts_with(marker) {
        return false;
    }
    let run = trimmed
        .chars()
        .take_while(|character| *character == marker)
        .count();
    run >= length && trimmed[run..].trim().is_empty()
}

fn push_block(lines: &mut Vec<String>, block: String) {
    if block.is_empty() {
        return;
    }
    lines.push(String::new());
    lines.push(block);
}

/// The reference `--include` inline body for one **exact** artifact payload.
///
/// This is the ONE inline formatter. It reads the same exact
/// [`sk_kernel::python_json::PythonJsonValue`] the `Full artifact:` block renders
/// (CF38's payload), so the inline lines and the payload can never disagree about
/// a value: an authored code point reaches the line verbatim instead of a scalar
/// projection of it, and no field is dropped to make a projection fit. Labels,
/// order, indentation and Python `str.strip()` are the reference formatters'
/// (`_format_inline_*_body` in `artifact_bodies.py`).
///
/// `model_steps` is the record's canonical typed model view for its `steps`
/// members (see [`include_body_lines_exact_with_steps`]); `None` keeps the
/// payload-only rendering for callers that hold no canonical graph.
#[must_use]
pub fn include_body_lines_exact(kind: &str, body: &PythonJsonValue) -> Vec<PythonString> {
    include_body_lines_exact_with_steps(kind, body, None)
}

/// The reference `--include` inline body, with the canonical typed model view.
///
/// The reference's `_format_inline_tactic_body` / `_format_inline_procedure_body`
/// append each step through `_append_step_lines(body_lines, step)`, which reads
/// the step's **model**: `getattr(step, "title", str(step))` plus a non-blank
/// `description` sub-line. That phase is deliberately NOT the profile formatter's
/// `title or description or str(step)`: an empty title stays the empty header and
/// a description never stands in for it. A model-shaped step therefore renders
/// from [`ModelObject`]; a member with no Source model keeps the payload
/// rendering (the reference's model validation refuses such a record, so it never
/// formats one).
#[must_use]
pub(crate) fn include_body_lines_exact_with_steps(
    kind: &str,
    body: &PythonJsonValue,
    model_steps: Option<&[crate::body::ModelObject]>,
) -> Vec<PythonString> {
    let mut lines = Vec::new();
    for (field, label) in include_string_fields(kind) {
        if let Some(text) = exact_string_field(body, field)
            .map(|text| exact_strip(&text))
            .filter(|text| !text.is_empty())
        {
            lines.push(prefixed(&format!("    {label}: "), &text));
        }
    }
    if kind == "agent_profile" {
        let roles = exact_string_items(body, "roles");
        if !roles.is_empty() {
            lines.push(prefixed("    Roles: ", &joined(&roles, ", ")));
        }
    }
    if kind == "skill" {
        // `_format_inline_skill_body`: description/form above, then the joined
        // triggers and the `expands_to.target` wrapper target.
        let triggers = exact_string_items(body, "triggers");
        if !triggers.is_empty() {
            lines.push(prefixed("    Triggers: ", &joined(&triggers, ", ")));
        }
        if let Some(target) = body
            .field("expands_to")
            .and_then(|expands| exact_string_field(expands, "target"))
            .map(|target| exact_strip(&target))
            .filter(|text| !text.is_empty())
        {
            lines.push(prefixed("    Expands to: ", &target));
        }
    }
    if kind == "glossary_pack" {
        // `_format_inline_glossary_pack_body` is the one inline formatter with a
        // two-space shape (its description and its `- surface: definition` terms),
        // deliberately unlike the 4/6-space `artifact_bodies` formatters. Its
        // strings are interpolated raw, never stripped.
        if let Some(description) =
            exact_string_field(body, "description").filter(|text| !text.is_empty())
        {
            lines.push(prefixed("  ", &description));
        }
        if let Some(PythonJsonValue::Array(terms)) = body.field("terms") {
            for term in terms {
                let surface = exact_string_field(term, "surface").unwrap_or_default();
                let definition = exact_string_field(term, "definition").unwrap_or_default();
                let mut line = prefixed("  - ", &surface);
                line.push_scalars(": ");
                line.append(&definition);
                lines.push(line);
            }
        }
    }
    if kind == "mission_step_contract" {
        // `_format_inline_step_contract_body`: each step is ONE line naming its id
        // before its description, unlike the tactic/procedure `title`/description split.
        let steps = match body.field("steps") {
            Some(PythonJsonValue::Array(steps)) if !steps.is_empty() => Some(steps),
            _ => None,
        };
        if let Some(steps) = steps {
            lines.push(PythonString::from_scalars("    Steps:"));
            for step in steps {
                let description = exact_string_field(step, "description").unwrap_or_default();
                match exact_string_field(step, "id") {
                    Some(id) => {
                        let mut line = prefixed("      - ", &id);
                        line.push_scalars(": ");
                        line.append(&description);
                        lines.push(line);
                    }
                    None => lines.push(prefixed("      - ", &description)),
                }
            }
        }
    }
    for (field, label) in include_list_fields(kind) {
        let Some(PythonJsonValue::Array(items)) = body.field(field) else {
            continue;
        };
        if items.is_empty() {
            continue;
        }
        lines.push(PythonString::from_scalars(&format!("    {label}:")));
        for (index, item) in items.iter().enumerate() {
            if *field == "steps"
                && let Some(step) = model_steps.and_then(|steps| steps.get(index))
            {
                lines.extend(include_step_lines(step));
                continue;
            }
            match item {
                PythonJsonValue::String(text) => lines.push(prefixed("      - ", text)),
                other => {
                    let title = exact_string_field(other, "title")
                        .or_else(|| exact_string_field(other, "id"));
                    let description = exact_string_field(other, "description")
                        .map(|text| exact_strip(&text))
                        .filter(|text| !text.is_empty());
                    match (title, description) {
                        (Some(title), Some(description)) => {
                            lines.push(prefixed("      - ", &title));
                            lines.push(prefixed("        ", &description));
                        }
                        (Some(title), None) => lines.push(prefixed("      - ", &title)),
                        (None, Some(description)) => lines.push(prefixed("      - ", &description)),
                        // The reference interpolates the item's own Python `str()`.
                        // This crate has no model `__str__`/`repr` authority, so the
                        // item's exact JSON text is the honest rendering; a
                        // model-shaped step always has a title or an id. Disclosed as
                        // a `repr` residual, never fabricated as a model repr.
                        (None, None) => lines.push(prefixed("      - ", &compact_json(other))),
                    }
                }
            }
        }
    }
    lines
}

/// The reference `--include` inline body for one finite scalar model dump.
///
/// This is the `serde_json` **capability adapter** over
/// [`include_body_lines_exact`]: the scalar tree is projected through the
/// canonical Kernel value model, so one formatter renders both inputs. A payload
/// `serde_json` cannot represent never reaches it — a caller that needs the whole
/// domain uses the exact API.
#[must_use]
pub fn include_body_lines(kind: &str, body: &serde_json::Value) -> Vec<String> {
    include_body_lines_exact(kind, &PythonJsonValue::from_serde(body))
        .into_iter()
        .map(|line| {
            line.to_scalar_string()
                .expect("a serde_json tree projects only scalar Python strings")
        })
        .collect()
}

/// One exact string member of an exact object payload.
fn exact_string_field(body: &PythonJsonValue, field: &str) -> Option<PythonString> {
    match body.field(field) {
        Some(PythonJsonValue::String(text)) => Some(text.clone()),
        _ => None,
    }
}

/// The exact string members of a list member (non-string items are skipped, like
/// a `list[str]` model field's own members).
fn exact_string_items(body: &PythonJsonValue, field: &str) -> Vec<PythonString> {
    match body.field(field) {
        Some(PythonJsonValue::Array(items)) => items
            .iter()
            .filter_map(|item| match item {
                PythonJsonValue::String(text) => Some(text.clone()),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// Python `str.strip()` over the exact code points of one authored scalar.
///
/// The one Python whitespace predicate the sibling payload lines use
/// (`sk_kernel::python_text::is_whitespace`), so an inline line never disagrees
/// with the payload about what counts as whitespace. Shared with the budget
/// module's heading/profile/selection splitting.
pub(crate) fn exact_strip(text: &PythonString) -> PythonString {
    let codepoints = text.codepoints();
    let start = codepoints
        .iter()
        .position(|code| !sk_kernel::python_text::is_whitespace(*code))
        .unwrap_or(codepoints.len());
    let end = codepoints
        .iter()
        .rposition(|code| !sk_kernel::python_text::is_whitespace(*code))
        .map_or(start, |position| position + 1);
    PythonString::from_units(codepoints[start..end].to_vec())
        .expect("a slice of validated code points is valid")
}

/// One line: `prefix` followed by the exact value.
fn prefixed(prefix: &str, value: &PythonString) -> PythonString {
    let mut line = PythonString::from_scalars(prefix);
    line.append(value);
    line
}

/// `separator.join(values)`, the reference's `', '.join(...)`.
pub(crate) fn joined(values: &[PythonString], separator: &str) -> PythonString {
    let mut out = PythonString::default();
    for (position, value) in values.iter().enumerate() {
        if position > 0 {
            out.push_scalars(separator);
        }
        out.append(value);
    }
    out
}

/// `json.dumps(value, ensure_ascii=True)`: the text for a value the reference
/// would interpolate through its own Python `str()`.
fn compact_json(value: &PythonJsonValue) -> PythonString {
    value.render_python_with(PythonJsonRenderOptions::default())
}

/// The reference `_append_step_lines` for one model-shaped step.
///
/// `step_title = getattr(step, "title", str(step))` and then
/// `f"      - {step_title}"`: the model always declares `title`, so its value is
/// the header (an empty title stays empty) and the `str(step)` default is only
/// the attribute-missing branch. A non-blank `description` follows as the
/// indented sub-line; `title.strip()` is not consulted here.
fn include_step_lines(step: &crate::body::ModelObject) -> Vec<PythonString> {
    let mut lines = Vec::new();
    // Python evaluates the getattr default before looking up `title`, even
    // when the attribute exists. Profile's `or` chain is deliberately lazy.
    let default = step.python_str();
    let title = step
        .field("title")
        .map_or(default, crate::body::ModelValue::python_str);
    lines.push(prefixed("      - ", &title));
    if let Some(description) = step
        .field("description")
        .and_then(crate::body::ModelValue::as_text)
    {
        let description = exact_strip(description);
        if !description.is_empty() {
            lines.push(prefixed("        ", &description));
        }
    }
    lines
}

/// Requested fields for the existing inline formatter, derived from its own
/// descriptors. Selected/global bodies must not dump unrelated Any members.
pub(crate) fn inline_body_field_names(kind: &str) -> Vec<&'static str> {
    let mut fields: Vec<&str> = include_string_fields(kind)
        .iter()
        .chain(include_list_fields(kind))
        .map(|(field, _)| *field)
        .collect();
    // This kind's step body is the dedicated id/description branch above,
    // not the generic labelled-list formatter.
    if kind == "mission_step_contract" {
        fields.push("steps");
    }
    fields
}

/// The scalar fields an `--include` inline body renders, in reference order.
fn include_string_fields(kind: &str) -> &'static [(&'static str, &'static str)] {
    match kind {
        "directive" => &[("intent", "Intent"), ("scope", "Scope")],
        "tactic" => &[("name", "Name"), ("purpose", "Purpose")],
        "styleguide" => &[("title", "Title"), ("scope", "Scope")],
        "toolguide" => &[("title", "Title"), ("tool", "Tool"), ("summary", "Summary")],
        "procedure" => &[
            ("name", "Name"),
            ("purpose", "Purpose"),
            ("entry_condition", "Entry condition"),
            ("exit_condition", "Exit condition"),
        ],
        "agent_profile" => &[("name", "Name"), ("purpose", "Purpose")],
        "paradigm" => &[("name", "Name"), ("summary", "Summary")],
        "skill" => &[("description", "Description"), ("form", "Form")],
        "mission_step_contract" => &[("action", "Action"), ("mission", "Mission")],
        _ => &[],
    }
}

/// The list fields an `--include` inline body renders, in reference order.
fn include_list_fields(kind: &str) -> &'static [(&'static str, &'static str)] {
    match kind {
        "directive" => &[
            ("procedures", "Procedures"),
            ("integrity_rules", "Integrity rules"),
            ("validation_criteria", "Validation criteria"),
        ],
        "styleguide" => &[("principles", "Principles")],
        "tactic" | "procedure" => &[("steps", "Steps")],
        _ => &[],
    }
}

/// The reference `Full artifact:` block for one **exact** artifact payload.
///
/// `json.dumps(payload, indent=2, sort_keys=True)` uses the Kernel's ONE Python
/// JSON encoder, including ASCII escapes, CPython float spellings and the
/// non-finite `NaN`/`Infinity`/`-Infinity` tokens the reference's default
/// `allow_nan=True` writes. The input is the exact payload
/// ([`crate::canonical_artifact_body_exact`]), so typed non-finite floats and
/// exact code points (a lone surrogate is escaped as `\uXXXX`) survive to the
/// printed text. A non-object or empty payload has no block.
#[must_use]
pub fn include_full_payload_lines_exact(
    payload: &sk_kernel::python_json::PythonJsonValue,
) -> Vec<String> {
    // Both object representations are the same value model: `ExactObject` is the
    // reader/`exact` payload shape and `Object` is the scalar convenience view a
    // `serde_json` adapter builds. Any other root has no block.
    let empty = match payload {
        sk_kernel::python_json::PythonJsonValue::Object(entries) => entries.is_empty(),
        sk_kernel::python_json::PythonJsonValue::ExactObject(object) => object.is_empty(),
        _ => return Vec::new(),
    };
    if empty {
        return Vec::new();
    }
    let document = payload
        .render_python_with(
            sk_kernel::python_json::PythonJsonRenderOptions::new(Some(2)).sort_keys(true),
        )
        .to_scalar_string()
        .expect("ensure_ascii=True renders only scalar ASCII");
    let mut lines = vec!["    Full artifact:".to_owned()];
    lines.extend(document.lines().map(|line| format!("      {line}")));
    lines
}

/// The reference `Full artifact:` block for one finite scalar model dump.
///
/// This is the `serde_json` **capability adapter** over
/// [`include_full_payload_lines_exact`]: the scalar tree is projected through the
/// canonical Kernel value model, so one encoder renders both inputs and no
/// second formatter exists. A payload `serde_json` cannot represent never gets
/// here — [`crate::canonical_artifact_body`] refuses it upstream. A non-object
/// or empty payload has no block.
#[must_use]
pub fn include_full_payload_lines(payload: &serde_json::Value) -> Vec<String> {
    include_full_payload_lines_exact(&sk_kernel::python_json::PythonJsonValue::from_serde(
        payload,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugging_matches_the_section_selector_contract() {
        assert_eq!(
            slugify_heading("Regression Vigilance"),
            "regression-vigilance"
        );
        assert_eq!(
            slugify_heading("  Code Review  Checklist "),
            "code-review-checklist"
        );
        assert_eq!(
            critical_section_selector("Terminology Canon"),
            "section:terminology-canon"
        );
    }

    #[test]
    fn policy_summary_prefers_the_authored_section() {
        let markdown = "# Charter\n\n- noisy\n\n## Policy Summary\n\n- Rule one\n- Rule two\n\n## Other\n- ignored\n";
        assert_eq!(
            extract_policy_summary(markdown),
            vec!["Rule one".to_owned(), "Rule two".to_owned()]
        );
    }

    #[test]
    fn policy_summary_falls_back_to_the_first_bullets() {
        let markdown = "- one\n- two\n- three\n";
        assert_eq!(
            extract_policy_summary(markdown),
            vec!["one".to_owned(), "two".to_owned(), "three".to_owned()]
        );
    }

    #[test]
    fn section_body_stops_at_the_next_same_level_heading() {
        let markdown = "## One\ntext\n### Nested\nmore\n## Two\ntail\n";
        assert_eq!(
            extract_section_body(markdown, "One").as_deref(),
            Some("text\n### Nested\nmore")
        );
    }

    #[test]
    fn fenced_headings_do_not_split_a_section() {
        let markdown = "## One\n```\n## Not a heading\n```\nstill one\n## Two\n";
        assert_eq!(
            extract_section_body(markdown, "One").as_deref(),
            Some("```\n## Not a heading\n```\nstill one")
        );
    }

    #[test]
    fn missing_critical_section_renders_the_fetch_rail() {
        let block = render_critical_section_bodies("# Empty\n", "implement");
        assert!(block.starts_with("Action-Critical Charter Sections (implement):"));
        assert!(
            block.contains("Run: spec-kitty charter context --include section:terminology-canon")
        );
        assert!(
            block.contains("When you rename or introduce a term in the diff, run this command")
        );
    }

    #[test]
    fn unrecognized_section_selector_is_not_rendered() {
        assert!(render_section_include("# Charter\n", "unknown-slug").is_none());
        assert!(
            render_section_include("# Charter\n", "regression-vigilance")
                .unwrap()
                .contains("has not yet authored a **Regression Vigilance** section")
        );
    }

    #[test]
    fn when_clause_normalization_matches_the_closed_lead_in_set() {
        assert_eq!(
            normalize_when_clause("are about to ship"),
            "are about to ship"
        );
        assert_eq!(
            normalize_when_clause("designing a page"),
            DEFAULT_WHEN_CLAUSE
        );
        assert_eq!(
            normalize_when_clause("when the boundary moves."),
            "need to the boundary moves"
        );
        assert_eq!(
            normalize_when_clause("when assessing tests."),
            DEFAULT_WHEN_CLAUSE
        );
        assert_eq!(normalize_when_clause("  "), DEFAULT_WHEN_CLAUSE);
    }

    #[test]
    fn include_helpers_project_a_model_dump_and_skip_empty_payloads() {
        // The reference `Full artifact:` block is absent for a non-object or empty dump.
        assert!(include_full_payload_lines(&serde_json::json!({})).is_empty());
        assert!(include_full_payload_lines(&serde_json::json!([])).is_empty());
        let block = include_full_payload_lines(&serde_json::json!({"b": 1, "a": "x"}));
        // Indented `json.dumps(..., indent=2, sort_keys=True)`.
        assert_eq!(
            block,
            vec![
                "    Full artifact:".to_owned(),
                "      {".to_owned(),
                "        \"a\": \"x\",".to_owned(),
                "        \"b\": 1".to_owned(),
                "      }".to_owned(),
            ]
        );
        // A kind with no inline formatter contributes no body lines.
        let unknown = serde_json::json!({"name": "x", "purpose": "y"});
        assert!(include_body_lines("unknown_kind", &unknown).is_empty());
        // A step's absent description omits the sub-line entirely.
        let tactic = serde_json::json!({"steps": [{"title": "Only title"}]});
        assert_eq!(
            include_body_lines("tactic", &tactic),
            vec!["    Steps:".to_owned(), "      - Only title".to_owned()]
        );
    }

    #[test]
    fn include_step_lines_read_the_canonical_step_model_when_one_exists() {
        // The reference `_append_step_lines` consumes the step *model*
        // (`getattr(step, "title", str(step))` + a non-blank description
        // sub-line). The model path must equal the payload path for every
        // model-shaped step, and it is the model — not the payload — that owns
        // the phase.
        let directory = tempfile::tempdir().expect("fixture directory");
        let path = directory.path().join("artifact.yaml");
        std::fs::write(
            &path,
            "id: probe-tactic\nschema_version: '1.0'\nname: Probe Tactic\nsteps:\n  - title: ''\n    description: described\n  - title: T\n",
        )
        .expect("fixture write");
        let document = crate::offering::value::ArtifactDocument::read(&path).expect("read");
        let payload = crate::body::exact_payload(sk_charter::ArtifactKind::Tactic, document.root())
            .expect("payload");
        let steps =
            crate::body::model_list(sk_charter::ArtifactKind::Tactic, document.root(), "steps")
                .expect("schema")
                .expect("model-shaped steps");
        let expected = vec![
            "    Name: Probe Tactic".to_owned(),
            "    Steps:".to_owned(),
            "      - ".to_owned(),
            "        described".to_owned(),
            "      - T".to_owned(),
        ];
        let from_payload = include_body_lines_exact("tactic", &payload)
            .iter()
            .map(|line| line.to_scalar_string().expect("scalar"))
            .collect::<Vec<_>>();
        let from_model = include_body_lines_exact_with_steps("tactic", &payload, Some(&steps))
            .iter()
            .map(|line| line.to_scalar_string().expect("scalar"))
            .collect::<Vec<_>>();
        assert_eq!(from_payload, expected);
        assert_eq!(from_model, expected);
    }
}
