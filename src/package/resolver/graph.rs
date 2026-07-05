use crate::types::{PackageFormat, PackageId, RepoSource};

#[derive(Debug, Clone, PartialEq)]
pub enum ConflictType {
    SystemInstalled,
    SameBatch,
}

#[derive(Debug, Clone)]
pub struct MetadataConflict {
    pub package: String,
    pub conflicts_with: String,
    pub conflict_type: ConflictType,
}

#[derive(Debug, Clone)]
pub struct ConflictSuggestion {
    pub kind: ConflictSuggestionKind,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ConflictSuggestionKind {
    Sandbox,
    ReplaceVersion,
    Skip,
}

#[derive(Debug, Clone, PartialEq)]
pub enum UnresolvedReason {
    NotInRepos,
    VersionConflict,
    CircularDependency,
    MissingProvider,
}

#[derive(Debug, Clone)]
pub struct UnresolvedDep {
    pub package: PackageId,
    pub reason: UnresolvedReason,
    pub details: String,
}

#[derive(Debug, Clone)]
pub struct Alternative {
    pub name: String,
    pub format: PackageFormat,
    pub version: String,
    pub source: RepoSource,
    pub priority: u32,
}

#[derive(Debug)]
pub struct ResolvedGraph {
    pub topological_order: Vec<PackageId>,
    pub cycles: Vec<Vec<PackageId>>,
    pub unresolved: Vec<PackageId>,
    pub unresolved_deps: Vec<UnresolvedDep>,
    pub metadata_conflicts: Vec<MetadataConflict>,
    pub alternatives: Vec<Alternative>,
}

impl Default for ResolvedGraph {
    fn default() -> Self {
        Self {
            topological_order: Vec::new(),
            cycles: Vec::new(),
            unresolved: Vec::new(),
            unresolved_deps: Vec::new(),
            metadata_conflicts: Vec::new(),
            alternatives: Vec::new(),
        }
    }
}

impl ResolvedGraph {
    pub fn new(topological_order: Vec<PackageId>) -> Self {
        Self {
            topological_order,
            ..Default::default()
        }
    }

    pub fn with_cycles(mut self, cycles: Vec<Vec<PackageId>>) -> Self {
        self.cycles = cycles;
        self
    }

    pub fn with_unresolved(mut self, unresolved: Vec<PackageId>) -> Self {
        self.unresolved = unresolved;
        self
    }

    pub fn with_metadata_conflicts(mut self, conflicts: Vec<MetadataConflict>) -> Self {
        self.metadata_conflicts = conflicts;
        self
    }

    pub fn with_alternatives(mut self, alternatives: Vec<Alternative>) -> Self {
        self.alternatives = alternatives;
        self
    }

    pub fn all_package_names(&self) -> Vec<&str> {
        self.topological_order.iter().map(|p| p.name.as_str()).collect()
    }
}
