//! Shared, hermetic fixtures for the `sk-install` acceptance tests.
//!
//! Everything is created under `tempfile` directories. No test touches the real
//! checkout, the user's home, the shared cache, or the network: pack sources are
//! `file://` repositories with explicit commit evidence, and archives are built
//! in-memory.

#![allow(dead_code)]

use std::path::{Path, PathBuf};

use sk_install::TREE_DIGEST_ALGORITHM;
use sk_install::agent::{CANONICAL_COMMANDS, PROMPT_BACKED_COMMANDS};
use sk_install::pin::{PackPin, TreeDigest};
use sk_install::render::TemplateSource;

/// Write `content` to `path`, creating parents.
pub fn write(path: &Path, content: impl AsRef<[u8]>) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create parent");
    }
    std::fs::write(path, content).expect("write fixture file");
}

/// Read `path` as UTF-8.
pub fn read(path: &Path) -> String {
    std::fs::read_to_string(path).expect("read fixture file")
}

/// A prompt template body that exercises every renderer path.
pub fn prompt_template(command: &str) -> String {
    format!(
        "---\ndescription: Run the {command} workflow\n---\n\n\
## Purpose\n\n\
Drive the {command} workflow for this project.\n\n\
## User Input\n\n\
```text\n$ARGUMENTS\n```\n\n\
## Steps\n\n\
1. Read /spec-kitty.plan for context.\n\
2. Run `spec-kitty {command} {{ARGS}}`.\n"
    )
}

/// A minimal prompt template that lacks the User Input block.
pub fn prompt_without_user_input(command: &str) -> String {
    format!("# {command}\n\n## Purpose\n\nDo the thing.\n")
}

/// Create a canonical missions root with a prompt for every prompt-backed command.
pub fn missions_root(root: &Path) -> TemplateSource {
    for command in PROMPT_BACKED_COMMANDS {
        write(
            &root
                .join("mission-steps/software-dev")
                .join(command)
                .join("prompt.md"),
            prompt_template(command),
        );
    }
    TemplateSource::new(root)
}

/// Create a missions root where one command's template is replaced.
pub fn missions_root_with(root: &Path, command: &str, content: &str) -> TemplateSource {
    let source = missions_root(root);
    write(
        &root
            .join("mission-steps/software-dev")
            .join(command)
            .join("prompt.md"),
        content,
    );
    source
}

/// All canonical commands, for coverage assertions.
pub fn canonical_commands() -> &'static [&'static str; 14] {
    &CANONICAL_COMMANDS
}

/// A `file://` pack repository holding one pinned subtree.
pub struct PackRepository {
    /// The repository root (the `file://` URL points here).
    pub root: PathBuf,
    /// The pack subtree inside the repository.
    pub pack_root: PathBuf,
    /// The pinned commit recorded in `COMMIT`.
    pub commit: String,
}

impl PackRepository {
    /// Build a repository whose `packs/built-in` tree is authored by `author`.
    pub fn create(root: &Path, commit: &str, author: impl FnOnce(&Path)) -> Self {
        let pack_root = root.join("packs/built-in");
        std::fs::create_dir_all(&pack_root).expect("create pack root");
        write(&pack_root.join("pack.yaml"), default_pack_yaml());
        write(
            &pack_root.join("directives/DIRECTIVE_001.yaml"),
            "id: DIRECTIVE_001\ntitle: Fixture\r\nbody: CRLF line\r\n",
        );
        write(&pack_root.join("root.graph.yaml"), "x: 1\n");
        author(&pack_root);
        write(&root.join("COMMIT"), format!("{commit}\n"));
        Self {
            root: root.to_path_buf(),
            pack_root,
            commit: commit.to_string(),
        }
    }

    /// The `file://` URL of this repository.
    pub fn url(&self) -> String {
        format!("file://{}", self.root.display())
    }

    /// A pin for this repository computed from the current tree.
    pub fn pin(&self) -> PackPin {
        let clean = sk_install::clean_tree_digest(&self.pack_root, sk_charter::PackTier::BuiltIn)
            .expect("clean digest");
        let full = sk_install::full_tree_digest(&self.pack_root).expect("full digest");
        PackPin::from_parts(
            Path::new("<fixture-pin>"),
            self.url(),
            self.commit.clone(),
            "packs/built-in".to_string(),
            TreeDigest {
                algorithm: TREE_DIGEST_ALGORITHM.to_string(),
                value: clean,
                level: "built-in".to_string(),
            },
            Some(">=4.0.0rc7,<5".to_string()),
            Some(full),
        )
        .expect("pin is valid")
    }
}

/// The canonical fixture `pack.yaml` (a built-in descriptor requires a version).
pub fn default_pack_yaml() -> &'static str {
    "pack_id: 01KZ6G6H000000000000000000\npack_version: 1.0.0\nname: fixture\npack_level: built-in\n"
}

/// Oracle-computed digests for the canonical fixture pack (see the crate test
/// that pins these against the Python reference implementation).
pub const ORACLE_CLEAN: &str = "ceab382fec1de836f428057fdde326724a4852884ccdf56846786541fb9099d3";
/// Oracle-computed full-tree digest for the canonical fixture pack.
pub const ORACLE_FULL: &str = "29e5c6e1004e79848701ac872a312183d0f98930a16037f50829b8cfeaafe137";
/// A syntactically valid commit used by fixtures.
pub const FIXTURE_COMMIT: &str = "a79c36318b7a506ca2ab26d74705c5e3e406004f";

/// Count entries in a generated manifest.
pub fn manifest_entry_count(project_root: &Path) -> usize {
    sk_install::AssetManifest::load(project_root)
        .expect("manifest loads")
        .expect("manifest exists")
        .entries()
        .len()
}

/// The bytes of a generated file (panics when absent).
pub fn file_bytes(path: &Path) -> Vec<u8> {
    std::fs::read(path).expect("fixture file exists")
}
