use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use sk_kernel::WpId;

use crate::error::MissionError;
use crate::work_package::WorkPackage;

/// A validated work-package dependency DAG for one Mission.
///
/// Construction refuses unknown dependencies, self-dependencies, duplicate
/// work-package ids, and cycles. The graph stores one deterministic topological
/// order (dependencies before dependents, ties broken by ascending id) so
/// callers never depend on directory iteration order.
///
/// The graph is a planning artifact only. It carries no mission state and does
/// not read or mutate `meta.json`, worktrees, or event logs.
#[derive(Clone, Debug)]
pub struct WorkPackageGraph {
    packages: BTreeMap<WpId, WorkPackage>,
    dependents: BTreeMap<WpId, Vec<WpId>>,
    order: Vec<WpId>,
}

impl WorkPackageGraph {
    /// Validate and build a graph from already-parsed work packages.
    pub fn build(packages: Vec<WorkPackage>) -> Result<Self, MissionError> {
        let mut by_id = BTreeMap::new();
        for package in packages {
            let id = package.id().clone();
            if by_id.insert(id.clone(), package).is_some() {
                return Err(MissionError::DuplicateWorkPackage { wp: id });
            }
        }

        for (id, package) in &by_id {
            for dependency in package.dependencies() {
                if dependency == id {
                    return Err(MissionError::SelfDependency { wp: id.clone() });
                }
                if !by_id.contains_key(dependency) {
                    return Err(MissionError::UnknownDependency {
                        wp: id.clone(),
                        dependency: dependency.clone(),
                    });
                }
            }
        }

        detect_cycle(&by_id)?;

        let mut dependents: BTreeMap<WpId, Vec<WpId>> =
            by_id.keys().cloned().map(|id| (id, Vec::new())).collect();
        for (id, package) in &by_id {
            for dependency in package.dependencies() {
                if let Some(entries) = dependents.get_mut(dependency) {
                    entries.push(id.clone());
                }
            }
        }
        for entries in dependents.values_mut() {
            entries.sort();
            entries.dedup();
        }

        let order =
            kahn_order(&by_id, &dependents).ok_or_else(|| MissionError::DependencyCycle {
                cycle: "<cycle not reported>".to_owned(),
            })?;

        Ok(Self {
            packages: by_id,
            dependents,
            order,
        })
    }

    /// Parse every top-level `WP*.md` file in a `tasks` directory.
    ///
    /// An absent `tasks` directory yields an empty graph. Symlinked entries and
    /// subdirectories are refused/skipped respectively; parsing is recursive in
    /// neither case.
    pub fn from_tasks_dir(tasks_dir: &Path) -> Result<Self, MissionError> {
        match fs::symlink_metadata(tasks_dir) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(MissionError::SymlinkedEntry {
                    path: tasks_dir.to_path_buf(),
                });
            }
            Ok(metadata) if !metadata.is_dir() => {
                return Err(MissionError::TasksDirectoryNotDirectory {
                    path: tasks_dir.to_path_buf(),
                });
            }
            Ok(_) => {}
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
                return Self::build(Vec::new());
            }
            Err(source) => return Err(MissionError::io("inspect", tasks_dir, source)),
        }

        let mut entries: Vec<PathBuf> = Vec::new();
        for entry in
            fs::read_dir(tasks_dir).map_err(|source| MissionError::io("list", tasks_dir, source))?
        {
            let entry = entry.map_err(|source| MissionError::io("list", tasks_dir, source))?;
            let file_type = entry
                .file_type()
                .map_err(|source| MissionError::io("inspect", entry.path(), source))?;
            if file_type.is_symlink() {
                return Err(MissionError::SymlinkedEntry { path: entry.path() });
            }
            if !file_type.is_file() {
                continue;
            }
            let name = entry.file_name();
            let name = name
                .to_str()
                .ok_or_else(|| MissionError::NonUtf8FileName { path: entry.path() })?;
            if name.ends_with(".md") && crate::work_package::filename_wp_id(&entry.path()).is_some()
            {
                entries.push(entry.path());
            }
        }
        entries.sort();

        let mut packages = Vec::with_capacity(entries.len());
        for path in entries {
            packages.push(WorkPackage::from_file(&path)?);
        }
        Self::build(packages)
    }

    /// Look up one work package.
    pub fn get(&self, id: &WpId) -> Option<&WorkPackage> {
        self.packages.get(id)
    }

    /// Number of work packages in the graph.
    pub fn len(&self) -> usize {
        self.packages.len()
    }

    /// Whether the graph holds no work packages.
    pub fn is_empty(&self) -> bool {
        self.packages.is_empty()
    }

    /// Ids in ascending order.
    pub fn ids(&self) -> impl Iterator<Item = &WpId> {
        self.packages.keys()
    }

    /// Work packages in ascending id order.
    pub fn work_packages(&self) -> impl Iterator<Item = &WorkPackage> {
        self.packages.values()
    }

    /// Declared dependencies of `id`, in declaration order.
    pub fn dependencies_of(&self, id: &WpId) -> Option<&[WpId]> {
        self.packages.get(id).map(|package| package.dependencies())
    }

    /// Work packages that declare `id` as a dependency, ascending.
    pub fn dependents_of(&self, id: &WpId) -> Option<&[WpId]> {
        self.dependents.get(id).map(Vec::as_slice)
    }

    /// Deterministic topological order: every dependency precedes its dependents.
    pub fn topological_order(&self) -> &[WpId] {
        &self.order
    }
}

/// Depth-first cycle detection with deterministic traversal.
///
/// Reports the lexicographically first cycle found when starting from ascending
/// ids and visiting dependencies in ascending order.
fn detect_cycle(packages: &BTreeMap<WpId, WorkPackage>) -> Result<(), MissionError> {
    #[derive(Clone, Copy, Eq, PartialEq)]
    enum Color {
        White,
        Gray,
        Black,
    }

    let mut color: BTreeMap<&WpId, Color> = packages.keys().map(|id| (id, Color::White)).collect();
    let mut stack: Vec<&WpId> = Vec::new();

    fn visit<'a>(
        id: &'a WpId,
        packages: &'a BTreeMap<WpId, WorkPackage>,
        color: &mut BTreeMap<&'a WpId, Color>,
        stack: &mut Vec<&'a WpId>,
    ) -> Result<(), MissionError> {
        color.insert(id, Color::Gray);
        stack.push(id);

        let mut dependencies: Vec<&WpId> = packages
            .get(id)
            .map(|package| package.dependencies().iter().collect())
            .unwrap_or_default();
        dependencies.sort();

        for dependency in dependencies {
            match color.get(dependency).copied().unwrap_or(Color::White) {
                Color::Gray => {
                    let start = stack
                        .iter()
                        .position(|entry| *entry == dependency)
                        .unwrap_or(0);
                    let mut path: Vec<String> = stack[start..]
                        .iter()
                        .map(|entry| entry.to_string())
                        .collect();
                    path.push(dependency.to_string());
                    return Err(MissionError::DependencyCycle {
                        cycle: path.join(" -> "),
                    });
                }
                Color::White => visit(dependency, packages, color, stack)?,
                Color::Black => {}
            }
        }

        stack.pop();
        color.insert(id, Color::Black);
        Ok(())
    }

    let ids: Vec<&WpId> = packages.keys().collect();
    for id in ids {
        if color.get(id).copied() == Some(Color::White) {
            visit(id, packages, &mut color, &mut stack)?;
        }
    }
    Ok(())
}

/// Kahn's algorithm with ascending-id tie-breaking.
fn kahn_order(
    packages: &BTreeMap<WpId, WorkPackage>,
    dependents: &BTreeMap<WpId, Vec<WpId>>,
) -> Option<Vec<WpId>> {
    let mut remaining: BTreeMap<WpId, usize> = packages
        .iter()
        .map(|(id, package)| (id.clone(), package.dependencies().len()))
        .collect();
    let mut ready: BTreeSet<WpId> = remaining
        .iter()
        .filter(|(_, count)| **count == 0)
        .map(|(id, _)| id.clone())
        .collect();

    let mut order = Vec::with_capacity(packages.len());
    while let Some(id) = ready.pop_first() {
        order.push(id.clone());
        for dependent in dependents.get(&id).into_iter().flatten() {
            if let Some(count) = remaining.get_mut(dependent) {
                *count -= 1;
                if *count == 0 {
                    ready.insert(dependent.clone());
                }
            }
        }
    }

    (order.len() == packages.len()).then_some(order)
}
