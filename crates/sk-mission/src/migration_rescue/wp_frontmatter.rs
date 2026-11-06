//! Source-compatible WP frontmatter emission (Source `frontmatter.FrontmatterManager`).
//!
//! Every write these rescue migrations make to a `tasks/WP*.md` (and the
//! `tasks.md` strip) goes through ONE renderer, exactly like Source routes every
//! write through `FrontmatterManager.write`/`render`. Two Source facts drive it,
//! and both are data-shape differences a downstream reader sees:
//!
//! * a `WP*` document without `dependencies` gains `dependencies: []` on read
//!   (Source `read`: backward compatibility with pre-0.11.0 files), so the
//!   field is present in the emitted document;
//! * keys are emitted in `WP_FIELD_ORDER`, then every remaining key
//!   alphabetically (Source `_normalize_frontmatter`).
//!
//! The authored *value* blocks travel verbatim: this renderer reorders the
//! document's own top-level blocks, keeps each key's authored comments and
//! scalar/list style (a flow `[GH-1]` stays flow, a block list stays block), and
//! adds only the canonical empty `dependencies: []` line. It parses nothing
//! itself — the caller keeps using the one frontmatter parser and semantic
//! verifier — so there is no second YAML codec here.

use crate::migration_rescue::top_level_key_of;

/// Source `FrontmatterManager.WP_FIELD_ORDER` — the ONE canonical field order.
///
/// Mutable status fields are deliberately absent: they travel via the event log.
pub const WP_FIELD_ORDER: [&str; 18] = [
    "work_package_id",
    "title",
    "dependencies",
    "requirement_refs",
    "planning_base_branch",
    "merge_target_branch",
    "branch_strategy",
    "base_branch",
    "base_commit",
    "created_at",
    "subtasks",
    "phase",
    "assignee",
    "agent",
    "shell_pid",
    "shell_pid_created_at",
    "scope",
    "history",
];

/// The `dependencies` line Source injects into a `WP*` document that lacks it.
const DEPENDENCIES_EMPTY: &str = "dependencies: []";

/// One authored top-level block: its key (when it has one) and its raw text.
struct Block {
    key: Option<String>,
    text: String,
}

/// Split a frontmatter body into authored top-level blocks.
///
/// Lines before the first key (a header comment, a blank line) form a keyless
/// leading block, which stays at the top. A trailing comment or blank line after
/// a key belongs to that key's block, so reordering never drops or re-attaches
/// authored comments.
fn split_blocks(yaml: &str) -> Vec<Block> {
    let mut blocks: Vec<Block> = Vec::new();
    let mut pending = String::new();
    let mut current: Option<Block> = None;
    for line in yaml.split_inclusive('\n') {
        let content = line.trim_end_matches(['\r', '\n']);
        match top_level_key_of(content) {
            Some(key) => {
                if let Some(block) = current.take() {
                    blocks.push(block);
                }
                current = Some(Block {
                    key: Some(key.to_string()),
                    text: std::mem::take(&mut pending),
                });
                if let Some(block) = current.as_mut() {
                    block.text.push_str(line);
                }
            }
            None => match current.as_mut() {
                Some(block) => block.text.push_str(line),
                None => pending.push_str(line),
            },
        }
    }
    if let Some(block) = current.take() {
        blocks.push(block);
    }
    if !pending.is_empty() {
        blocks.insert(
            0,
            Block {
                key: None,
                text: pending,
            },
        );
    }
    blocks
}

/// Emit `yaml` in Source's canonical order, injecting `dependencies: []` for a
/// work-package document that lacks it.
pub fn render_normalized(yaml: &str, is_work_package: bool) -> String {
    let blocks = split_blocks(yaml);
    let mut head = String::new();
    let mut by_key: Vec<(String, String)> = Vec::new();
    for block in blocks {
        match block.key {
            Some(key) => by_key.push((key, block.text)),
            None => head.push_str(&block.text),
        }
    }
    let has_dependencies = by_key.iter().any(|(key, _)| key == "dependencies");

    let mut order: Vec<String> = Vec::new();
    for field in WP_FIELD_ORDER {
        let authored = by_key.iter().any(|(key, _)| key == field);
        let inject = is_work_package && field == "dependencies" && !has_dependencies;
        if authored || inject {
            order.push(field.to_string());
        }
    }
    let mut remaining: Vec<String> = by_key
        .iter()
        .map(|(key, _)| key.clone())
        .filter(|key| !order.contains(key))
        .collect();
    remaining.sort();
    order.extend(remaining);

    let mut out = String::with_capacity(yaml.len() + 24);
    out.push_str(&head);
    for key in &order {
        if key == "dependencies" && is_work_package && !has_dependencies {
            out.push_str(DEPENDENCIES_EMPTY);
            out.push('\n');
            continue;
        }
        if let Some((_, text)) = by_key.iter().find(|(candidate, _)| candidate == key) {
            out.push_str(text);
            if !text.ends_with('\n') {
                out.push('\n');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_work_package_gains_an_empty_dependencies_list_in_source_order() {
        let authored = concat!(
            "work_package_id: 01WP\n",
            "title: alpha\n",
            "authoritative_surface: src/foo/\n",
            "execution_mode: code_change\n",
            "mission_id: 01MC\n",
            "owned_files:\n",
            "- src/foo/bar.py\n",
            "tracker_refs: [GH-1]\n",
            "wp_code: WP01\n",
        );
        assert_eq!(
            render_normalized(authored, true),
            concat!(
                "work_package_id: 01WP\n",
                "title: alpha\n",
                "dependencies: []\n",
                "authoritative_surface: src/foo/\n",
                "execution_mode: code_change\n",
                "mission_id: 01MC\n",
                "owned_files:\n",
                "- src/foo/bar.py\n",
                "tracker_refs: [GH-1]\n",
                "wp_code: WP01\n",
            )
        );
    }

    #[test]
    fn authored_comments_and_flow_style_survive_the_reorder() {
        let authored = concat!(
            "title: alpha  # keep me\n",
            "# a leading note for the surface\n",
            "owned_files: [src/a.py]\n",
            "notes: |\n",
            "  two lines\n",
        );
        assert_eq!(
            render_normalized(authored, true),
            // The authored comment stays attached to the block it followed
            // (`title`), and the injected `dependencies` line lands after that
            // block in Source's field order.
            concat!(
                "title: alpha  # keep me\n",
                "# a leading note for the surface\n",
                "dependencies: []\n",
                "notes: |\n",
                "  two lines\n",
                "owned_files: [src/a.py]\n",
            )
        );
    }

    #[test]
    fn a_non_work_package_gets_no_dependencies_line() {
        let authored = "title: tasks\nlane: approved\n";
        assert_eq!(
            render_normalized(authored, false),
            "title: tasks\nlane: approved\n"
        );
    }

    #[test]
    fn an_authored_dependencies_value_is_kept_verbatim() {
        let authored = "title: x\ndependencies:\n- WP01\n";
        assert_eq!(
            render_normalized(authored, true),
            "title: x\ndependencies:\n- WP01\n"
        );
    }
}
