//! Strict graph-document loading and additive layer merge.
//!
//! Mirrors `src/charter/offering/drg/loader.py`:
//!
//! * `graph.yaml` wins when present, otherwise `*.graph.yaml` fragments merge
//!   left-to-right in filename order;
//! * an unknown top-level key fails closed at the load boundary
//!   (`DRGGraphSchemaError`), never through a silently-degrading reader;
//! * [`merge_layers`] is additive: a project node with an existing URN may
//!   replace the label only, never the built-in kind.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_yaml_ng::Value;

use crate::error::DoctrineError;
use crate::graph::{DrgGraph, DrgNode};

const TOP_LEVEL_KEYS: [&str; 5] = [
    "schema_version",
    "generated_at",
    "generated_by",
    "nodes",
    "edges",
];
const FRAGMENT_SUFFIX: &str = ".graph.yaml";
const SINGLE_GRAPH: &str = "graph.yaml";

/// Read one graph document.
pub fn load_graph(path: &Path) -> Result<DrgGraph, DoctrineError> {
    if !path.exists() {
        return Err(DoctrineError::GraphNotFound {
            path: path.to_path_buf(),
        });
    }
    let text = std::fs::read_to_string(path)
        .map_err(|source| DoctrineError::io("read graph", path, source))?;
    load_graph_str(&text, path)
}

/// Parse one graph document from text, attributing diagnostics to `origin`.
pub fn load_graph_str(text: &str, origin: &Path) -> Result<DrgGraph, DoctrineError> {
    let value: Value =
        serde_yaml_ng::from_str(text).map_err(|error| DoctrineError::InvalidYaml {
            path: origin.to_path_buf(),
            message: error.to_string(),
        })?;
    let Value::Mapping(mapping) = &value else {
        return Err(DoctrineError::InvalidGraphDocument {
            path: origin.to_path_buf(),
            message: "expected a YAML mapping at top level".to_owned(),
        });
    };
    let unknown: Vec<String> = mapping
        .keys()
        .filter_map(Value::as_str)
        .filter(|key| !TOP_LEVEL_KEYS.contains(key))
        .map(str::to_owned)
        .collect();
    if !unknown.is_empty() {
        return Err(DoctrineError::UnknownTopLevelKey {
            path: origin.to_path_buf(),
            keys: unknown,
        });
    }
    let graph: DrgGraph = serde_yaml_ng::from_value(value).map_err(|error| {
        let message = error.to_string();
        if let Some(field) = unknown_field(&message) {
            DoctrineError::UnknownField {
                path: origin.to_path_buf(),
                location: location_from_message(&message),
                field,
            }
        } else {
            DoctrineError::InvalidGraphDocument {
                path: origin.to_path_buf(),
                message,
            }
        }
    })?;
    Ok(graph)
}

fn unknown_field(message: &str) -> Option<String> {
    let marker = "unknown field `";
    let start = message.find(marker)? + marker.len();
    let rest = &message[start..];
    let end = rest.find('`')?;
    Some(rest[..end].to_owned())
}

fn location_from_message(message: &str) -> String {
    let first_line = message.lines().next().unwrap_or(message);
    if first_line.contains("nodes") {
        "node".to_owned()
    } else if first_line.contains("edges") {
        "edge".to_owned()
    } else {
        "graph document".to_owned()
    }
}

/// Whether `path` holds a graph source the loader recognises.
pub fn has_graph_files(path: &Path) -> bool {
    if !path.is_dir() {
        return false;
    }
    if path.join(SINGLE_GRAPH).is_file() {
        return true;
    }
    graph_fragments(path).is_ok_and(|fragments| !fragments.is_empty())
}

fn graph_fragments(directory: &Path) -> Result<Vec<PathBuf>, DoctrineError> {
    let entries = std::fs::read_dir(directory)
        .map_err(|source| DoctrineError::io("read graph directory", directory, source))?;
    let mut fragments = Vec::new();
    for entry in entries {
        let entry =
            entry.map_err(|source| DoctrineError::io("read graph directory", directory, source))?;
        let path = entry.path();
        if path.is_file()
            && path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(FRAGMENT_SUFFIX))
        {
            fragments.push(path);
        }
    }
    fragments.sort();
    Ok(fragments)
}

/// Load a graph from a file, or from a graph directory.
///
/// A directory prefers `graph.yaml`; otherwise its `*.graph.yaml` fragments are
/// merged left-to-right with [`merge_layers`].
pub fn load_graph_or_dir(path: &Path) -> Result<DrgGraph, DoctrineError> {
    if path.is_file() {
        return load_graph(path);
    }
    if !path.exists() {
        return Err(DoctrineError::GraphNotFound {
            path: path.to_path_buf(),
        });
    }
    if !path.is_dir() {
        return Err(DoctrineError::NotAGraphSource {
            path: path.to_path_buf(),
        });
    }
    let single = path.join(SINGLE_GRAPH);
    if single.is_file() {
        return load_graph(&single);
    }
    let fragments = graph_fragments(path)?;
    let Some((first, rest)) = fragments.split_first() else {
        return Err(DoctrineError::NoGraphFiles {
            path: path.to_path_buf(),
        });
    };
    let mut graph = load_graph(first)?;
    for fragment in rest {
        graph = merge_layers(&graph, Some(&load_graph(fragment)?))?;
    }
    Ok(graph)
}

/// Additively merge two graph layers.
///
/// All built-in nodes are kept. A project node with a new URN is added; a
/// project node whose URN already exists may override the label only (the
/// built-in kind is retained). All edges from both layers are combined.
pub fn merge_layers(
    built_in: &DrgGraph,
    project: Option<&DrgGraph>,
) -> Result<DrgGraph, DoctrineError> {
    let Some(project) = project else {
        return Ok(built_in.clone());
    };
    let mut nodes: Vec<DrgNode> = built_in.nodes().to_vec();
    let mut index: BTreeMap<String, usize> = nodes
        .iter()
        .enumerate()
        .map(|(position, node)| (node.urn().to_owned(), position))
        .collect();
    for node in project.nodes() {
        match index.get(node.urn()).copied() {
            Some(position) => {
                if node.label().is_some() {
                    nodes[position] = nodes[position]
                        .clone()
                        .with_label(node.label().map(str::to_owned));
                }
            }
            None => {
                index.insert(node.urn().to_owned(), nodes.len());
                nodes.push(node.clone());
            }
        }
    }
    let mut edges = built_in.edges().to_vec();
    edges.extend(project.edges().iter().cloned());
    built_in.with_parts(nodes, edges)
}
