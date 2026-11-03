//! Support for the committed scoped-safe-commit oracle corpus.
//!
//! This module is included by `tests/safe_commit_conformance.rs` only. It has
//! three jobs:
//!
//! 1. a dependency-free JSON reader/writer for the committed fixture shape;
//! 2. the shared scenario runner/capture used by the ignored subprocess helper
//!    (so the case executes in its own process, which is the only safe way to
//!    exercise an inherited `GIT_INDEX_FILE` and a post-CAS competitor);
//! 3. the comparison of the oracle snapshot with the native snapshot.
//!
//! The Python generator (`tests/fixtures/safe_commit/generate_oracle.py`)
//! writes the same snapshot structure, so every comparison is between two real
//! observations, never between a frozen expectation constant and itself.

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use sk_git::{
    BranchName, CommitGuardCapability, GitError, GitPath, GitRepository, Oid, ProtectedBranches,
    SafeCommitLandedKind, SafeCommitOutcome, SafeCommitRequest,
};
use sk_kernel::content_digest::sha256_hex;

// ---------------------------------------------------------------------------
// Minimal JSON
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Int(i64),
    Str(String),
    Arr(Vec<Json>),
    Obj(BTreeMap<String, Json>),
}

impl Json {
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut parser = Parser {
            chars: text.chars().peekable(),
            offset: 0,
        };
        let value = parser.value()?;
        parser.skip_ws();
        if parser.chars.next().is_some() {
            return Err(format!("trailing JSON at char {}", parser.offset));
        }
        Ok(value)
    }

    pub fn to_text(&self) -> String {
        let mut out = String::new();
        write_value(self, &mut out);
        out
    }

    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(map) => map.get(key),
            _ => None,
        }
    }

    pub fn at(&self, key: &str) -> &Json {
        self.get(key)
            .unwrap_or_else(|| panic!("missing JSON field {key:?}"))
    }

    pub fn as_str(&self) -> &str {
        match self {
            Json::Str(value) => value,
            other => panic!("expected JSON string, got {other:?}"),
        }
    }

    pub fn as_opt_str(&self) -> Option<&str> {
        match self {
            Json::Null => None,
            Json::Str(value) => Some(value),
            other => panic!("expected JSON string or null, got {other:?}"),
        }
    }

    pub fn as_array(&self) -> &[Json] {
        match self {
            Json::Arr(values) => values,
            other => panic!("expected JSON array, got {other:?}"),
        }
    }

    pub fn as_bool(&self) -> bool {
        match self {
            Json::Bool(value) => *value,
            other => panic!("expected JSON bool, got {other:?}"),
        }
    }

    pub fn as_int(&self) -> i64 {
        match self {
            Json::Int(value) => *value,
            other => panic!("expected JSON int, got {other:?}"),
        }
    }

    pub fn strings(&self) -> Vec<String> {
        self.as_array()
            .iter()
            .map(|value| value.as_str().to_owned())
            .collect()
    }
}

pub fn json_object(entries: impl IntoIterator<Item = (String, Json)>) -> Json {
    Json::Obj(entries.into_iter().collect())
}

pub fn json_str(value: impl Into<String>) -> Json {
    Json::Str(value.into())
}

pub fn json_opt_str(value: Option<impl Into<String>>) -> Json {
    match value {
        Some(value) => Json::Str(value.into()),
        None => Json::Null,
    }
}

pub fn json_strings(values: &[String]) -> Json {
    Json::Arr(values.iter().map(|value| json_str(value.clone())).collect())
}

fn write_value(value: &Json, out: &mut String) {
    match value {
        Json::Null => out.push_str("null"),
        Json::Bool(true) => out.push_str("true"),
        Json::Bool(false) => out.push_str("false"),
        Json::Int(number) => out.push_str(&number.to_string()),
        Json::Str(text) => write_string(text, out),
        Json::Arr(values) => {
            out.push('[');
            for (index, value) in values.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_value(value, out);
            }
            out.push(']');
        }
        Json::Obj(map) => {
            out.push('{');
            for (index, (key, value)) in map.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_string(key, out);
                out.push(':');
                write_value(value, out);
            }
            out.push('}');
        }
    }
}

fn write_string(text: &str, out: &mut String) {
    out.push('"');
    for character in text.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            character if (character as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", character as u32));
            }
            character => out.push(character),
        }
    }
    out.push('"');
}

struct Parser<'a> {
    chars: std::iter::Peekable<std::str::Chars<'a>>,
    offset: usize,
}

impl Parser<'_> {
    fn skip_ws(&mut self) {
        while let Some(character) = self.chars.peek().copied() {
            if character.is_whitespace() {
                self.chars.next();
                self.offset += character.len_utf8();
            } else {
                break;
            }
        }
    }

    fn next(&mut self) -> Result<char, String> {
        let character = self
            .chars
            .next()
            .ok_or_else(|| "unexpected end of JSON".to_string())?;
        self.offset += character.len_utf8();
        Ok(character)
    }

    fn value(&mut self) -> Result<Json, String> {
        self.skip_ws();
        match self
            .chars
            .peek()
            .ok_or_else(|| "unexpected end of JSON".to_string())?
        {
            '"' => Ok(Json::Str(self.string()?)),
            '{' => self.object(),
            '[' => self.array(),
            't' => {
                self.expect_word("true")?;
                Ok(Json::Bool(true))
            }
            'f' => {
                self.expect_word("false")?;
                Ok(Json::Bool(false))
            }
            'n' => {
                self.expect_word("null")?;
                Ok(Json::Null)
            }
            character if *character == '-' || character.is_ascii_digit() => self.number(),
            other => Err(format!("unexpected JSON character {other:?}")),
        }
    }

    fn expect_word(&mut self, word: &str) -> Result<(), String> {
        for expected in word.chars() {
            let actual = self.next()?;
            if actual != expected {
                return Err(format!("expected {word:?} in JSON"));
            }
        }
        Ok(())
    }

    fn string(&mut self) -> Result<String, String> {
        if self.next()? != '"' {
            return Err("expected JSON string".to_string());
        }
        let mut value = String::new();
        loop {
            match self.next()? {
                '"' => return Ok(value),
                '\\' => match self.next()? {
                    '"' => value.push('"'),
                    '\\' => value.push('\\'),
                    '/' => value.push('/'),
                    'b' => value.push('\u{08}'),
                    'f' => value.push('\u{0c}'),
                    'n' => value.push('\n'),
                    'r' => value.push('\r'),
                    't' => value.push('\t'),
                    'u' => {
                        let first = self.hex4()?;
                        let code_point = if (0xD800..=0xDBFF).contains(&first) {
                            if self.next()? != '\\' || self.next()? != 'u' {
                                return Err("lone high surrogate in JSON".to_string());
                            }
                            let second = self.hex4()?;
                            if !(0xDC00..=0xDFFF).contains(&second) {
                                return Err("bad low surrogate in JSON".to_string());
                            }
                            0x10000 + ((first - 0xD800) << 10) + (second - 0xDC00)
                        } else {
                            first
                        };
                        value.push(
                            char::from_u32(code_point)
                                .ok_or_else(|| "invalid JSON code point".to_string())?,
                        );
                    }
                    other => return Err(format!("bad JSON escape {other:?}")),
                },
                character => value.push(character),
            }
        }
    }

    fn hex4(&mut self) -> Result<u32, String> {
        let mut value = 0u32;
        for _ in 0..4 {
            let character = self.next()?;
            let digit = character
                .to_digit(16)
                .ok_or_else(|| "bad JSON hex escape".to_string())?;
            value = value * 16 + digit;
        }
        Ok(value)
    }

    fn number(&mut self) -> Result<Json, String> {
        let mut text = String::new();
        while let Some(character) = self.chars.peek().copied() {
            if character.is_ascii_digit() || character == '-' || character == '+' {
                text.push(character);
                self.chars.next();
                self.offset += character.len_utf8();
            } else {
                break;
            }
        }
        text.parse::<i64>()
            .map(Json::Int)
            .map_err(|_| format!("unsupported JSON number {text:?}"))
    }

    fn array(&mut self) -> Result<Json, String> {
        self.next()?;
        let mut values = Vec::new();
        self.skip_ws();
        if self.chars.peek() == Some(&']') {
            self.next()?;
            return Ok(Json::Arr(values));
        }
        loop {
            values.push(self.value()?);
            self.skip_ws();
            match self.next()? {
                ',' => continue,
                ']' => return Ok(Json::Arr(values)),
                other => return Err(format!("unexpected JSON array separator {other:?}")),
            }
        }
    }

    fn object(&mut self) -> Result<Json, String> {
        self.next()?;
        let mut map = BTreeMap::new();
        self.skip_ws();
        if self.chars.peek() == Some(&'}') {
            self.next()?;
            return Ok(Json::Obj(map));
        }
        loop {
            self.skip_ws();
            let key = self.string()?;
            self.skip_ws();
            if self.next()? != ':' {
                return Err("expected ':' in JSON object".to_string());
            }
            let value = self.value()?;
            map.insert(key, value);
            self.skip_ws();
            match self.next()? {
                ',' => continue,
                '}' => return Ok(Json::Obj(map)),
                other => return Err(format!("unexpected JSON object separator {other:?}")),
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Fixture model
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileEntry {
    pub path: String,
    pub sha256: String,
    pub text: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomIndex {
    pub exists: bool,
    pub entries: Vec<String>,
    pub index_flags: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub branch: Option<String>,
    pub head: String,
    pub refs: Vec<String>,
    pub status: Vec<String>,
    pub index: Vec<String>,
    pub index_flags: Vec<String>,
    pub files: Vec<FileEntry>,
    pub message_sha256: String,
    pub message: Option<String>,
    pub tree: String,
    pub parents: Vec<String>,
    pub committed_paths: Vec<String>,
    pub reflog: Vec<String>,
    pub custom_index: Option<CustomIndex>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookEvidence {
    pub hook: String,
    pub nargs: i64,
    pub argv: Vec<String>,
    pub index: String,
    pub index_in_git_dir: String,
    pub cwd_is_worktree: String,
    pub git_dir: String,
    pub git_prefix: String,
    pub git_editor: String,
    pub status: Vec<String>,
    pub cached: Vec<String>,
    pub extra: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OracleOutcome {
    pub outcome: String,
    pub refusal_class: Option<String>,
    pub facts: Vec<String>,
    pub commit: Option<String>,
    pub landed_kind: Option<String>,
    pub warnings: Vec<String>,
    pub error_detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeExpectation {
    pub variant: Option<String>,
    pub landed_kind: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    Write { path: String, content: String },
    Argv(Vec<String>),
    Shell(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetupStep {
    pub custom_index: bool,
    pub step: Step,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookSpec {
    pub name: String,
    pub script: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Setup {
    pub branch: String,
    pub files: Vec<(String, String)>,
    pub steps: Vec<SetupStep>,
    pub hooks: Vec<HookSpec>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub message: String,
    pub paths: Vec<String>,
    pub index_deletions: Vec<String>,
    pub expected_parent: Option<String>,
    pub custom_index: bool,
    pub wait_for_sentinel: bool,
    pub protection: Option<Vec<String>>,
    pub capability: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Divergence {
    pub kind: String,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Case {
    pub name: String,
    pub description: String,
    pub route: String,
    pub expectation: String,
    pub divergence: Option<Divergence>,
    pub native: NativeExpectation,
    pub detail_tokens: Vec<String>,
    pub setup: Setup,
    pub request: Request,
    pub before: Snapshot,
    pub after: Snapshot,
    pub hooks: Vec<HookEvidence>,
    pub oracle: OracleOutcome,
}

impl Case {
    pub fn from_json(value: &Json) -> Self {
        Case {
            name: value.at("name").as_str().to_owned(),
            description: value.at("description").as_str().to_owned(),
            route: value.at("route").as_str().to_owned(),
            expectation: value.at("expectation").as_str().to_owned(),
            divergence: match value.at("divergence") {
                Json::Null => None,
                Json::Obj(_) => Some(Divergence {
                    kind: value.at("divergence").at("kind").as_str().to_owned(),
                    detail: value.at("divergence").at("detail").as_str().to_owned(),
                }),
                other => panic!("bad divergence: {other:?}"),
            },
            native: NativeExpectation {
                variant: value
                    .at("native")
                    .at("variant")
                    .as_opt_str()
                    .map(str::to_owned),
                landed_kind: value
                    .at("native")
                    .at("landed_kind")
                    .as_opt_str()
                    .map(str::to_owned),
            },
            detail_tokens: value.at("detail_tokens").strings(),
            setup: parse_setup(value.at("setup")),
            request: parse_request(value.at("request")),
            before: Snapshot::from_json(value.at("before")),
            after: Snapshot::from_json(value.at("after")),
            hooks: value
                .at("hooks")
                .as_array()
                .iter()
                .map(HookEvidence::from_json)
                .collect(),
            oracle: OracleOutcome::from_json(value.at("oracle")),
        }
    }
}

fn parse_setup(value: &Json) -> Setup {
    Setup {
        branch: value.at("branch").as_str().to_owned(),
        files: value
            .at("files")
            .as_array()
            .iter()
            .map(|entry| {
                (
                    entry.at("path").as_str().to_owned(),
                    entry.at("content").as_str().to_owned(),
                )
            })
            .collect(),
        steps: value
            .at("steps")
            .as_array()
            .iter()
            .map(|entry| SetupStep {
                custom_index: entry
                    .get("custom_index")
                    .map(Json::as_bool)
                    .unwrap_or(false),
                step: if let Some(write) = entry.get("write") {
                    Step::Write {
                        path: write.at("path").as_str().to_owned(),
                        content: write.at("content").as_str().to_owned(),
                    }
                } else if let Some(argv) = entry.get("argv") {
                    Step::Argv(argv.strings())
                } else if let Some(shell) = entry.get("shell") {
                    Step::Shell(shell.as_str().to_owned())
                } else {
                    panic!("unknown setup step {entry:?}")
                },
            })
            .collect(),
        hooks: value
            .at("hooks")
            .as_array()
            .iter()
            .map(|entry| HookSpec {
                name: entry.at("name").as_str().to_owned(),
                script: entry.at("script").as_str().to_owned(),
            })
            .collect(),
    }
}

fn parse_request(value: &Json) -> Request {
    Request {
        message: value.at("message").as_str().to_owned(),
        paths: value.at("paths").strings(),
        index_deletions: value.at("index_deletions").strings(),
        expected_parent: value.at("expected_parent").as_opt_str().map(str::to_owned),
        custom_index: value.at("custom_index").as_bool(),
        wait_for_sentinel: value.at("wait_for_sentinel").as_bool(),
        protection: match value.at("protection") {
            Json::Null => None,
            other => Some(other.strings()),
        },
        capability: value.at("capability").as_str().to_owned(),
    }
}

impl Snapshot {
    pub fn from_json(value: &Json) -> Self {
        Snapshot {
            branch: value.at("branch").as_opt_str().map(str::to_owned),
            head: value.at("head").as_str().to_owned(),
            refs: value.at("refs").strings(),
            status: value.at("status").strings(),
            index: value.at("index").strings(),
            index_flags: value.at("index_flags").strings(),
            files: value
                .at("files")
                .as_array()
                .iter()
                .map(|entry| FileEntry {
                    path: entry.at("path").as_str().to_owned(),
                    sha256: entry.at("sha256").as_str().to_owned(),
                    text: entry.get("text").map(Json::as_str).map(str::to_owned),
                })
                .collect(),
            message_sha256: value.at("message_sha256").as_str().to_owned(),
            message: value.at("message").as_opt_str().map(str::to_owned),
            tree: value.at("tree").as_str().to_owned(),
            parents: value.at("parents").strings(),
            committed_paths: value.at("committed_paths").strings(),
            reflog: value.at("reflog").strings(),
            custom_index: match value.at("custom_index") {
                Json::Null => None,
                custom => Some(CustomIndex {
                    exists: custom.at("exists").as_bool(),
                    entries: custom.at("entries").strings(),
                    index_flags: custom.at("index_flags").strings(),
                }),
            },
        }
    }

    pub fn to_json(&self) -> Json {
        json_object([
            ("branch".to_string(), json_opt_str(self.branch.clone())),
            ("head".to_string(), json_str(self.head.clone())),
            ("refs".to_string(), json_strings(&self.refs)),
            ("status".to_string(), json_strings(&self.status)),
            ("index".to_string(), json_strings(&self.index)),
            ("index_flags".to_string(), json_strings(&self.index_flags)),
            (
                "files".to_string(),
                Json::Arr(
                    self.files
                        .iter()
                        .map(|entry| {
                            let mut fields = vec![
                                ("path".to_string(), json_str(entry.path.clone())),
                                ("sha256".to_string(), json_str(entry.sha256.clone())),
                            ];
                            if let Some(text) = &entry.text {
                                fields.push(("text".to_string(), json_str(text.clone())));
                            }
                            json_object(fields)
                        })
                        .collect(),
                ),
            ),
            (
                "message_sha256".to_string(),
                json_str(self.message_sha256.clone()),
            ),
            ("message".to_string(), json_opt_str(self.message.clone())),
            ("tree".to_string(), json_str(self.tree.clone())),
            ("parents".to_string(), json_strings(&self.parents)),
            (
                "committed_paths".to_string(),
                json_strings(&self.committed_paths),
            ),
            ("reflog".to_string(), json_strings(&self.reflog)),
            (
                "custom_index".to_string(),
                match &self.custom_index {
                    None => Json::Null,
                    Some(custom) => json_object([
                        ("exists".to_string(), Json::Bool(custom.exists)),
                        ("entries".to_string(), json_strings(&custom.entries)),
                        ("index_flags".to_string(), json_strings(&custom.index_flags)),
                    ]),
                },
            ),
        ])
    }
}

impl HookEvidence {
    fn from_json(value: &Json) -> Self {
        HookEvidence {
            hook: value.at("hook").as_str().to_owned(),
            nargs: value.at("nargs").as_int(),
            argv: value.at("argv").strings(),
            index: value.at("index").as_str().to_owned(),
            index_in_git_dir: value.at("index_in_git_dir").as_str().to_owned(),
            cwd_is_worktree: value.at("cwd_is_worktree").as_str().to_owned(),
            git_dir: value.at("git_dir").as_str().to_owned(),
            git_prefix: value.at("git_prefix").as_str().to_owned(),
            git_editor: value.at("git_editor").as_str().to_owned(),
            status: value.at("status").strings(),
            cached: value.at("cached").strings(),
            extra: value.at("extra").strings(),
        }
    }

    pub fn to_json(&self) -> Json {
        json_object([
            ("hook".to_string(), json_str(self.hook.clone())),
            ("nargs".to_string(), Json::Int(self.nargs)),
            ("argv".to_string(), json_strings(&self.argv)),
            ("index".to_string(), json_str(self.index.clone())),
            (
                "index_in_git_dir".to_string(),
                json_str(self.index_in_git_dir.clone()),
            ),
            (
                "cwd_is_worktree".to_string(),
                json_str(self.cwd_is_worktree.clone()),
            ),
            ("git_dir".to_string(), json_str(self.git_dir.clone())),
            ("git_prefix".to_string(), json_str(self.git_prefix.clone())),
            ("git_editor".to_string(), json_str(self.git_editor.clone())),
            ("status".to_string(), json_strings(&self.status)),
            ("cached".to_string(), json_strings(&self.cached)),
            ("extra".to_string(), json_strings(&self.extra)),
        ])
    }
}

impl OracleOutcome {
    fn from_json(value: &Json) -> Self {
        OracleOutcome {
            outcome: value.at("outcome").as_str().to_owned(),
            refusal_class: value.at("refusal_class").as_opt_str().map(str::to_owned),
            facts: value.at("facts").strings(),
            commit: value.at("commit").as_opt_str().map(str::to_owned),
            landed_kind: value.at("landed_kind").as_opt_str().map(str::to_owned),
            warnings: value.at("warnings").strings(),
            error_detail: value.at("error_detail").as_opt_str().map(str::to_owned),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeOutcome {
    pub outcome: String,
    pub refusal_class: Option<String>,
    pub facts: Vec<String>,
    pub variant: Option<String>,
    pub commit: Option<String>,
    pub landed_kind: Option<String>,
    pub warnings: Vec<String>,
    pub error_detail: Option<String>,
    pub candidate: Option<String>,
    pub observed_tip: Option<String>,
}

impl NativeOutcome {
    pub fn to_json(&self) -> Json {
        json_object([
            ("outcome".to_string(), json_str(self.outcome.clone())),
            (
                "refusal_class".to_string(),
                json_opt_str(self.refusal_class.clone()),
            ),
            ("facts".to_string(), json_strings(&self.facts)),
            ("variant".to_string(), json_opt_str(self.variant.clone())),
            ("commit".to_string(), json_opt_str(self.commit.clone())),
            (
                "landed_kind".to_string(),
                json_opt_str(self.landed_kind.clone()),
            ),
            ("warnings".to_string(), json_strings(&self.warnings)),
            (
                "error_detail".to_string(),
                json_opt_str(self.error_detail.clone()),
            ),
            (
                "candidate".to_string(),
                json_opt_str(self.candidate.clone()),
            ),
            (
                "observed_tip".to_string(),
                json_opt_str(self.observed_tip.clone()),
            ),
        ])
    }

    fn from_json(value: &Json) -> Self {
        NativeOutcome {
            outcome: value.at("outcome").as_str().to_owned(),
            refusal_class: value.at("refusal_class").as_opt_str().map(str::to_owned),
            facts: value.at("facts").strings(),
            variant: value.at("variant").as_opt_str().map(str::to_owned),
            commit: value.at("commit").as_opt_str().map(str::to_owned),
            landed_kind: value.at("landed_kind").as_opt_str().map(str::to_owned),
            warnings: value.at("warnings").strings(),
            error_detail: value.at("error_detail").as_opt_str().map(str::to_owned),
            candidate: value.at("candidate").as_opt_str().map(str::to_owned),
            observed_tip: value.at("observed_tip").as_opt_str().map(str::to_owned),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeRun {
    pub case: String,
    pub before: Snapshot,
    pub after: Snapshot,
    pub hooks: Vec<HookEvidence>,
    pub native: NativeOutcome,
}

impl NativeRun {
    pub fn to_json(&self) -> Json {
        json_object([
            ("case".to_string(), json_str(self.case.clone())),
            ("before".to_string(), self.before.to_json()),
            ("after".to_string(), self.after.to_json()),
            (
                "hooks".to_string(),
                Json::Arr(self.hooks.iter().map(HookEvidence::to_json).collect()),
            ),
            ("native".to_string(), self.native.to_json()),
        ])
    }

    pub fn from_json(value: &Json) -> Self {
        NativeRun {
            case: value.at("case").as_str().to_owned(),
            before: Snapshot::from_json(value.at("before")),
            after: Snapshot::from_json(value.at("after")),
            hooks: value
                .at("hooks")
                .as_array()
                .iter()
                .map(HookEvidence::from_json)
                .collect(),
            native: NativeOutcome::from_json(value.at("native")),
        }
    }
}

// ---------------------------------------------------------------------------
// Repository execution and capture
// ---------------------------------------------------------------------------

const SELECTOR_ENV: &[&str] = &[
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_COMMON_DIR",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_PREFIX",
    "GIT_CEILING_DIRECTORIES",
    "GIT_DISCOVERY_ACROSS_FILESYSTEM",
    "GIT_NAMESPACE",
    "GIT_SHALLOW_FILE",
    "GIT_GRAFT_FILE",
    "GIT_REPLACE_REF_BASE",
];

pub struct RunPaths {
    pub repo: PathBuf,
    pub custom_index: PathBuf,
    pub hook_log: PathBuf,
    pub sentinel: PathBuf,
    pub failed: PathBuf,
}

impl RunPaths {
    pub fn new(root: &Path) -> Self {
        RunPaths {
            repo: root.join("repo"),
            custom_index: root.join("custom-index"),
            hook_log: root.join("hooks.log"),
            sentinel: root.join("competitor.done"),
            failed: root.join("competitor.failed"),
        }
    }
}

fn base_command(program: &str, cwd: &Path, custom_index: Option<&Path>) -> Command {
    let mut command = Command::new(program);
    command.current_dir(cwd).stdin(Stdio::null());
    for selector in SELECTOR_ENV {
        command.env_remove(selector);
    }
    if let Some(index) = custom_index {
        command.env("GIT_INDEX_FILE", index);
    }
    command
}

fn run_git(cwd: &Path, args: &[&str], custom_index: Option<&Path>) -> Output {
    let output = base_command("git", cwd, custom_index)
        .args(args)
        .output()
        .unwrap_or_else(|error| panic!("could not run git {args:?} in {}: {error}", cwd.display()));
    if !output.status.success() {
        panic!(
            "git {args:?} failed in {}: {}\n{}",
            cwd.display(),
            String::from_utf8_lossy(&output.stderr),
            String::from_utf8_lossy(&output.stdout)
        );
    }
    output
}

fn git_text(cwd: &Path, args: &[&str], custom_index: Option<&Path>) -> String {
    String::from_utf8_lossy(&run_git(cwd, args, custom_index).stdout).into_owned()
}

fn git_bytes(cwd: &Path, args: &[&str], custom_index: Option<&Path>) -> Vec<u8> {
    run_git(cwd, args, custom_index).stdout
}

fn split_z(raw: &[u8]) -> Vec<&[u8]> {
    raw.split(|byte| *byte == 0)
        .filter(|record| !record.is_empty())
        .collect()
}

fn status_entries(repo: &Path, custom_index: Option<&Path>) -> Vec<String> {
    let raw = git_bytes(
        repo,
        &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
        custom_index,
    );
    let records = split_z(&raw);
    let mut entries = Vec::new();
    let mut index = 0;
    while index < records.len() {
        let record = records[index];
        let code = String::from_utf8_lossy(&record[..2]).into_owned();
        let path = String::from_utf8_lossy(&record[3..]).into_owned();
        if (code.starts_with('R') || code.starts_with('C')) && index + 1 < records.len() {
            let old = String::from_utf8_lossy(records[index + 1]).into_owned();
            entries.push(format!("{code} {path} <- {old}"));
            index += 2;
            continue;
        }
        entries.push(format!("{code} {path}"));
        index += 1;
    }
    entries
}

fn index_entries(repo: &Path, custom_index: Option<&Path>) -> Vec<String> {
    let raw = git_bytes(repo, &["ls-files", "-s", "-z"], custom_index);
    split_z(&raw)
        .into_iter()
        .map(|record| {
            let mut text = String::from_utf8_lossy(record).into_owned();
            if let Some(position) = text.find('\t') {
                text.replace_range(position..=position, " ");
            }
            text
        })
        .collect()
}

fn index_flags(repo: &Path, custom_index: Option<&Path>) -> Vec<String> {
    let raw = git_bytes(repo, &["ls-files", "-v", "-z"], custom_index);
    split_z(&raw)
        .into_iter()
        .map(|record| String::from_utf8_lossy(record).into_owned())
        .collect()
}

fn worktree_files(repo: &Path) -> Vec<FileEntry> {
    let mut paths = Vec::new();
    collect_files(repo, Path::new(""), &mut paths);
    paths.sort_by(|left, right| left.0.cmp(&right.0));
    paths
        .into_iter()
        .map(|(relative, absolute)| {
            let data = fs::read(&absolute)
                .unwrap_or_else(|error| panic!("could not read {}: {error}", absolute.display()));
            FileEntry {
                path: relative,
                sha256: sha256_hex(&data),
                text: String::from_utf8(data)
                    .ok()
                    .filter(|text| text.len() <= 8192),
            }
        })
        .collect()
}

fn collect_files(repo: &Path, relative: &Path, out: &mut Vec<(String, PathBuf)>) {
    let directory = repo.join(relative);
    let mut entries: Vec<_> = fs::read_dir(&directory)
        .unwrap_or_else(|error| panic!("could not read {}: {error}", directory.display()))
        .map(|entry| entry.expect("directory entry"))
        .collect();
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let name = entry.file_name();
        if name == OsStr::new(".git") {
            continue;
        }
        let child = relative.join(&name);
        let file_type = entry.file_type().expect("file type");
        if file_type.is_dir() {
            collect_files(repo, &child, out);
        } else if file_type.is_file() {
            out.push((child.to_string_lossy().into_owned(), repo.join(&child)));
        }
    }
}

fn reflog_entries(repo: &Path, branch: &str, custom_index: Option<&Path>) -> Vec<String> {
    let raw = git_text(
        repo,
        &[
            "reflog",
            "show",
            "--format=%H%x00%gs",
            &format!("refs/heads/{branch}"),
        ],
        custom_index,
    );
    raw.lines()
        .filter(|line| !line.is_empty())
        .map(|line| {
            let (sha, subject) = line.split_once('\x00').unwrap_or((line, ""));
            format!("{sha}|{subject}")
        })
        .collect()
}

fn commit_message(repo: &Path, sha: &str, custom_index: Option<&Path>) -> (String, Option<String>) {
    let raw = git_bytes(repo, &["cat-file", "commit", sha], custom_index);
    let message = match raw.windows(2).position(|window| window == b"\n\n") {
        Some(position) => raw[position + 2..].to_vec(),
        None => Vec::new(),
    };
    (sha256_hex(&message), String::from_utf8(message).ok())
}

fn snapshot(repo: &Path, branch: &str, custom_index: Option<&Path>, custom_view: bool) -> Snapshot {
    let head = git_text(repo, &["rev-parse", "HEAD"], None)
        .trim()
        .to_owned();
    let symbolic = base_command("git", repo, None)
        .args(["symbolic-ref", "--short", "-q", "HEAD"])
        .output()
        .expect("symbolic-ref");
    let current = if symbolic.status.success() {
        Some(String::from_utf8_lossy(&symbolic.stdout).trim().to_owned())
    } else {
        None
    };
    let mut refs: Vec<String> = git_text(
        repo,
        &["for-each-ref", "--format=%(refname) %(objectname)"],
        None,
    )
    .lines()
    .filter(|line| !line.is_empty())
    .map(str::to_owned)
    .collect();
    refs.sort();
    let parents_line = git_text(repo, &["rev-list", "--parents", "-n", "1", &head], None);
    let parents: Vec<String> = parents_line
        .split_whitespace()
        .skip(1)
        .map(str::to_owned)
        .collect();
    let tree = git_text(repo, &["rev-parse", &format!("{head}^{{tree}}")], None)
        .trim()
        .to_owned();
    let mut committed: Vec<String> = split_z(&git_bytes(
        repo,
        &[
            "show",
            "--name-only",
            "--format=",
            "-z",
            "--no-renames",
            &head,
        ],
        None,
    ))
    .into_iter()
    .map(|record| String::from_utf8_lossy(record).into_owned())
    .collect();
    committed.sort();
    let (message_sha256, message) = commit_message(repo, &head, None);
    Snapshot {
        branch: current.clone(),
        head,
        refs,
        status: status_entries(repo, None),
        index: index_entries(repo, None),
        index_flags: index_flags(repo, None),
        files: worktree_files(repo),
        message_sha256,
        message,
        tree,
        parents,
        committed_paths: committed,
        reflog: reflog_entries(repo, current.as_deref().unwrap_or(branch), None),
        custom_index: if custom_view {
            let exists = custom_index.is_some_and(Path::exists);
            Some(CustomIndex {
                exists,
                entries: if exists {
                    index_entries(repo, custom_index)
                } else {
                    Vec::new()
                },
                index_flags: if exists {
                    index_flags(repo, custom_index)
                } else {
                    Vec::new()
                },
            })
        } else {
            None
        },
    }
}

fn parse_hook_log(path: &Path) -> Vec<HookEvidence> {
    if !path.exists() {
        return Vec::new();
    }
    let text = fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("could not read hook log {}: {error}", path.display()));
    let mut hooks: Vec<HookEvidence> = Vec::new();
    for line in text.lines() {
        if let Some(name) = line.strip_prefix("hook=") {
            hooks.push(HookEvidence {
                hook: name.to_owned(),
                nargs: 0,
                argv: Vec::new(),
                index: "unset".to_string(),
                index_in_git_dir: "no".to_string(),
                cwd_is_worktree: "no".to_string(),
                git_dir: "<unset>".to_string(),
                git_prefix: String::new(),
                git_editor: "unset".to_string(),
                status: Vec::new(),
                cached: Vec::new(),
                extra: Vec::new(),
            });
            continue;
        }
        let Some(current) = hooks.last_mut() else {
            continue;
        };
        if let Some(value) = line.strip_prefix("status=") {
            current.status.push(value.to_owned());
            continue;
        }
        if let Some(value) = line.strip_prefix("cached=") {
            current.cached.push(value.to_owned());
            continue;
        }
        if line.starts_with("argv") && line.contains('=') {
            let (_, value) = line.split_once('=').expect("argv line");
            current.argv.push(value.to_owned());
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            current.extra.push(line.to_owned());
            continue;
        };
        match key {
            "nargs" => current.nargs = value.parse().expect("nargs"),
            "index" => current.index = value.to_owned(),
            "index_in_git_dir" => current.index_in_git_dir = value.to_owned(),
            "cwd_is_worktree" => current.cwd_is_worktree = value.to_owned(),
            "git_dir" => current.git_dir = value.to_owned(),
            "git_prefix" => current.git_prefix = value.to_owned(),
            "git_editor" => current.git_editor = value.to_owned(),
            other => current.extra.push(format!("{other}={value}")),
        }
    }
    hooks
}

fn apply_steps(repo: &Path, steps: &[SetupStep], custom_index: &Path) {
    for step in steps {
        let index = if step.custom_index {
            Some(custom_index)
        } else {
            None
        };
        match &step.step {
            Step::Write { path, content } => {
                let target = repo.join(path);
                if let Some(parent) = target.parent() {
                    fs::create_dir_all(parent).unwrap_or_else(|error| {
                        panic!("could not create {}: {error}", parent.display())
                    });
                }
                fs::write(&target, content).unwrap_or_else(|error| {
                    panic!("could not write {}: {error}", target.display())
                });
            }
            Step::Argv(argv) => {
                let Some(program) = argv.first() else {
                    panic!("empty setup argv");
                };
                let output = base_command(program, repo, index)
                    .args(&argv[1..])
                    .output()
                    .unwrap_or_else(|error| panic!("could not run {argv:?}: {error}"));
                assert!(
                    output.status.success(),
                    "setup command {argv:?} failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
            }
            Step::Shell(script) => {
                let output = base_command("sh", repo, index)
                    .args(["-eu", "-c", script])
                    .output()
                    .unwrap_or_else(|error| panic!("could not run setup shell: {error}"));
                assert!(
                    output.status.success(),
                    "setup shell failed: {}\n{}",
                    String::from_utf8_lossy(&output.stderr),
                    String::from_utf8_lossy(&output.stdout)
                );
            }
        }
    }
}

fn classify_native(error: &GitError) -> NativeOutcome {
    let detail = error.to_string();
    let (class, variant, commit, landed_kind, candidate, observed_tip) = match error {
        GitError::SafeCommitNothingToCommit { .. } => (
            "no_change",
            "SafeCommitNothingToCommit",
            None,
            None,
            None,
            None,
        ),
        GitError::SafeCommitDirectoryArgument { .. } => (
            "directory_argument",
            "SafeCommitDirectoryArgument",
            None,
            None,
            None,
            None,
        ),
        GitError::SafeCommitPathPolicy { .. } => (
            "path_policy",
            "SafeCommitPathPolicy",
            None,
            None,
            None,
            None,
        ),
        GitError::SafeCommitPathOutsideWorktree { .. } => (
            "path_outside_worktree",
            "SafeCommitPathOutsideWorktree",
            None,
            None,
            None,
            None,
        ),
        GitError::SafeCommitPathLoop { .. } => {
            ("path_loop", "SafeCommitPathLoop", None, None, None, None)
        }
        GitError::SafeCommitProtectedBranch { .. } => (
            "protected_branch",
            "SafeCommitProtectedBranch",
            None,
            None,
            None,
            None,
        ),
        GitError::SafeCommitHeadMismatch { .. } => (
            "head_mismatch",
            "SafeCommitHeadMismatch",
            None,
            None,
            None,
            None,
        ),
        GitError::SafeCommitDestinationMissing { .. } => (
            "destination_missing",
            "SafeCommitDestinationMissing",
            None,
            None,
            None,
            None,
        ),
        GitError::SafeCommitEmptyChangeset => (
            "empty_changeset",
            "SafeCommitEmptyChangeset",
            None,
            None,
            None,
            None,
        ),
        GitError::SafeCommitIndexDeletionConflict { .. } => (
            "index_deletion_conflict",
            "SafeCommitIndexDeletionConflict",
            None,
            None,
            None,
            None,
        ),
        GitError::SafeCommitInvalidRequest { .. } => (
            "invalid_request",
            "SafeCommitInvalidRequest",
            None,
            None,
            None,
            None,
        ),
        GitError::SafeCommitStageFailed { .. } => (
            "stage_failure",
            "SafeCommitStageFailed",
            None,
            None,
            None,
            None,
        ),
        GitError::SafeCommitAdvanceUnconfirmed {
            candidate: found,
            observed_tip: tip,
            ..
        } => (
            "advance_conflict",
            "SafeCommitAdvanceUnconfirmed",
            None,
            None,
            Some(found.to_string()),
            tip.as_ref().map(ToString::to_string),
        ),
        GitError::SafeCommitCommitFailed { detail, .. } => {
            let lowered = detail.to_lowercase();
            let class = if lowered.contains("empty commit message") {
                "empty_message"
            } else if lowered.contains("gpg failed to sign") || lowered.contains("signer") {
                "signing_failure"
            } else {
                "commit_failed"
            };
            (class, "SafeCommitCommitFailed", None, None, None, None)
        }
        GitError::SafeCommitHookRejected { .. } => (
            "hook_rejection",
            "SafeCommitHookRejected",
            None,
            None,
            None,
            None,
        ),
        GitError::SafeCommitHookChangedPaths => (
            "hook_changed_paths",
            "SafeCommitHookChangedPaths",
            None,
            None,
            None,
            None,
        ),
        GitError::SafeCommitExpectedBytesMismatch { .. } => (
            "expected_bytes_mismatch",
            "SafeCommitExpectedBytesMismatch",
            None,
            None,
            None,
            None,
        ),
        GitError::SafeCommitLanded { kind, recovery, .. } => {
            let kind_text = match kind {
                SafeCommitLandedKind::IndexResyncFailed => "IndexResyncFailed",
                SafeCommitLandedKind::IndexResidue => "IndexResidue",
                SafeCommitLandedKind::UnrequestedPaths => "UnrequestedPaths",
                SafeCommitLandedKind::CommitPathReadbackFailed => "CommitPathReadbackFailed",
            };
            (
                "landed_with_followup",
                "SafeCommitLanded",
                Some(recovery.commit().to_string()),
                Some(kind_text.to_string()),
                None,
                None,
            )
        }
        GitError::RepositoryEnvironmentRefused { .. } => (
            "environment_unsupported",
            "RepositoryEnvironmentRefused",
            None,
            None,
            None,
            None,
        ),
        GitError::OperationInProgress { .. } => (
            "operation_in_progress",
            "OperationInProgress",
            None,
            None,
            None,
            None,
        ),
        _ => ("other_variant", "other", None, None, None, None),
    };
    NativeOutcome {
        outcome: "error".to_string(),
        refusal_class: Some(class.to_string()),
        facts: Vec::new(),
        variant: Some(variant.to_string()),
        commit,
        landed_kind,
        warnings: Vec::new(),
        error_detail: Some(detail),
        candidate,
        observed_tip,
    }
}

pub fn execute_case(case: &Case, root: &Path) -> NativeRun {
    let paths = RunPaths::new(root);
    for directory in ["home", "tmp", "xdg-config", "xdg-cache", "xdg-data"] {
        fs::create_dir_all(root.join(directory)).expect("create isolation directory");
    }
    fs::create_dir_all(&paths.repo).expect("create fixture repository");
    let branch = &case.setup.branch;
    run_git(&paths.repo, &["init", "-q", "-b", branch], None);
    run_git(&paths.repo, &["config", "user.name", "Test User"], None);
    run_git(
        &paths.repo,
        &["config", "user.email", "test@example.com"],
        None,
    );
    for (path, content) in &case.setup.files {
        let target = paths.repo.join(path);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).expect("create fixture parent");
        }
        fs::write(&target, content).expect("write fixture file");
    }
    apply_steps(&paths.repo, &case.setup.steps, &paths.custom_index);
    if !case.setup.hooks.is_empty() {
        let hooks = paths.repo.join(".git").join("hooks");
        fs::create_dir_all(&hooks).expect("create hooks directory");
        for hook in &case.setup.hooks {
            let target = hooks.join(&hook.name);
            fs::write(&target, &hook.script).expect("write hook");
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&target, fs::Permissions::from_mode(0o755))
                    .expect("chmod hook");
            }
        }
    }

    let before = snapshot(
        &paths.repo,
        branch,
        if case.request.custom_index {
            Some(&paths.custom_index)
        } else {
            None
        },
        case.request.custom_index,
    );

    let repository = GitRepository::open(&paths.repo).expect("open fixture repository");
    let mut request = SafeCommitRequest::new(
        BranchName::parse(branch).expect("branch name"),
        case.request.message.clone(),
        case.request
            .paths
            .iter()
            .map(|path| GitPath::from_git_bytes(path.as_bytes()).expect("git path")),
    );
    if !case.request.index_deletions.is_empty() {
        request = request.with_index_deletions(
            case.request
                .index_deletions
                .iter()
                .map(|path| GitPath::from_git_bytes(path.as_bytes()).expect("git path")),
        );
    }
    if case.request.expected_parent.as_deref() == Some("head") {
        let head = git_text(&paths.repo, &["rev-parse", "HEAD"], None)
            .trim()
            .to_owned();
        request = request.with_expected_parent(Oid::parse(&head).expect("head oid"));
    }
    if let Some(protected) = &case.request.protection {
        assert_eq!(case.request.capability, "standard");
        request = request.with_protection(
            ProtectedBranches::parse(protected.iter().map(String::as_str))
                .expect("protected branches"),
            CommitGuardCapability::Standard,
        );
    }

    let mut native = match repository.safe_commit(&request) {
        Ok(outcome) => native_success(&outcome),
        Err(error) => classify_native(&error),
    };
    native.facts = case
        .detail_tokens
        .iter()
        .filter(|token| {
            native
                .error_detail
                .as_deref()
                .is_some_and(|detail| detail.contains(token.as_str()))
        })
        .map(|token| format!("detail:{token}"))
        .collect();

    if case.request.wait_for_sentinel {
        wait_for_sentinel(&paths);
    }

    let after = snapshot(
        &paths.repo,
        branch,
        if case.request.custom_index {
            Some(&paths.custom_index)
        } else {
            None
        },
        case.request.custom_index,
    );

    NativeRun {
        case: case.name.clone(),
        before,
        after,
        hooks: parse_hook_log(&paths.hook_log),
        native,
    }
}

fn native_success(outcome: &SafeCommitOutcome) -> NativeOutcome {
    let warnings = outcome
        .warnings()
        .iter()
        .map(|warning| {
            if let Some((_, suffix)) = warning.split_once("hook reported on a successful commit: ")
            {
                format!("hook_stderr:{}", suffix.trim())
            } else if warning.contains("post-commit hook could not complete") {
                "post_commit_failed".to_string()
            } else {
                format!("warning:{warning}")
            }
        })
        .collect();
    NativeOutcome {
        outcome: "ok".to_string(),
        refusal_class: None,
        facts: Vec::new(),
        variant: None,
        commit: Some(outcome.commit().to_string()),
        landed_kind: None,
        warnings,
        error_detail: None,
        candidate: None,
        observed_tip: None,
    }
}

fn wait_for_sentinel(paths: &RunPaths) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while std::time::Instant::now() < deadline {
        if paths.sentinel.exists() {
            return;
        }
        if paths.failed.exists() {
            panic!("post-CAS competitor helper reported failure");
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("post-CAS competitor helper did not signal completion within 30s");
}

// ---------------------------------------------------------------------------
// Comparison
// ---------------------------------------------------------------------------

pub fn assert_hook_evidence(oracle: &[HookEvidence], native: &[HookEvidence], case: &str) {
    assert_eq!(
        native.len(),
        oracle.len(),
        "{case}: hook invocation count differs (oracle {oracle:?}, native {native:?})"
    );
    for (index, (expected, actual)) in oracle.iter().zip(native).enumerate() {
        let context = format!("{case}: hook #{index} ({})", expected.hook);
        assert_eq!(actual.hook, expected.hook, "{context}: hook name");
        assert_eq!(actual.nargs, expected.nargs, "{context}: nargs");
        assert_eq!(actual.argv, expected.argv, "{context}: argv evidence");
        assert_eq!(
            actual.cwd_is_worktree, expected.cwd_is_worktree,
            "{context}: cwd"
        );
        assert_eq!(actual.git_dir, expected.git_dir, "{context}: GIT_DIR shape");
        assert_eq!(
            actual.status, expected.status,
            "{context}: candidate status"
        );
        assert_eq!(
            actual.cached, expected.cached,
            "{context}: candidate cached paths"
        );
        assert_eq!(actual.extra, expected.extra, "{context}: extra hook output");
        // Candidate index placement is an implementation detail, but the
        // hook's editor context is part of the route's actual git semantics.
        assert_eq!(
            actual.index, "private",
            "{context}: native hooks must see a private index"
        );
        assert_eq!(
            actual.index_in_git_dir, "yes",
            "{context}: native scoped index lives in the git directory"
        );
        assert_eq!(
            actual.git_editor, expected.git_editor,
            "{context}: hook editor context must match the oracle"
        );
        assert_eq!(
            actual.git_prefix, expected.git_prefix,
            "{context}: GIT_PREFIX shape (git sets it empty for hooks)"
        );
    }
}

pub fn assert_full_match(case: &Case, run: &NativeRun) {
    let context = &case.name;
    assert_eq!(
        run.before, case.before,
        "{context}: fixture setup drift (before state)"
    );
    assert_eq!(
        run.native.variant, case.native.variant,
        "{context}: typed native variant"
    );
    assert_eq!(
        run.native.landed_kind, case.native.landed_kind,
        "{context}: typed landed kind"
    );
    assert_eq!(
        run.native.outcome, case.oracle.outcome,
        "{context}: outcome (oracle {:?}, native {:?})",
        case.oracle, run.native
    );
    assert_eq!(
        run.native.refusal_class, case.oracle.refusal_class,
        "{context}: refusal class"
    );
    assert_eq!(
        run.native.facts, case.oracle.facts,
        "{context}: detail tokens"
    );
    assert_eq!(
        run.native.commit, case.oracle.commit,
        "{context}: landed commit"
    );
    assert_eq!(
        run.native.landed_kind, case.oracle.landed_kind,
        "{context}: oracle landed kind"
    );
    assert_eq!(
        run.native.warnings, case.oracle.warnings,
        "{context}: warnings"
    );
    assert_eq!(
        run.after, case.after,
        "{context}: after state differs from the oracle"
    );
    assert_hook_evidence(&case.hooks, &run.hooks, context);
}

/// Run one git read in a fixture repository with redirecting selectors removed.
pub fn git_stdout(repo: &Path, args: &[&str]) -> String {
    git_text(repo, args, None)
}

/// Whether `ancestor` is reachable from `descendant`.
pub fn is_ancestor(repo: &Path, ancestor: &str, descendant: &str) -> bool {
    base_command("git", repo, None)
        .args(["merge-base", "--is-ancestor", ancestor, descendant])
        .output()
        .expect("merge-base")
        .status
        .success()
}

/// Assert the transaction left no private index or lock behind.
pub fn assert_no_transient_git_state(repo: &Path, case: &str) {
    let git_dir = repo.join(".git");
    assert!(
        !git_dir.join("index.lock").exists(),
        "{case}: a git index lock was left behind"
    );
    for entry in fs::read_dir(&git_dir)
        .unwrap_or_else(|error| panic!("{case}: could not read {}: {error}", git_dir.display()))
    {
        let entry = entry.expect("git dir entry");
        let name = entry.file_name().to_string_lossy().into_owned();
        assert!(
            !(name.starts_with("spec-kitty-safe-commit")
                || name.starts_with("next-index-")
                || name.starts_with("spec-kitty-pin-cas")
                || name.starts_with("spec-kitty-index-deletions")),
            "{case}: transient private-index artifact {name} survived the transaction"
        );
    }
}
