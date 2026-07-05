use crate::error::SpmResult;
use crate::package::solver;
use crate::types::{PackageId, RepoSource, VersionStrategy};

#[derive(Debug, Clone)]
pub struct MetadataConflict {
    pub package: String,
    pub conflicts_with: String,
    pub conflict_type: ConflictType,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ConflictType {
    SystemInstalled,
    SameBatch,
}

#[derive(Debug, Default)]
pub struct ResolvedGraph {
    pub topological_order: Vec<PackageId>,
    pub cycles: Vec<Vec<PackageId>>,
    pub unresolved: Vec<PackageId>,
    pub metadata_conflicts: Vec<MetadataConflict>,
}

pub fn bump_dep_cache_epoch() {}

pub fn resolve_dependencies(
    pid: &PackageId,
    strategy: VersionStrategy,
) -> SpmResult<ResolvedGraph> {
    resolve_dependencies_preferred(pid, strategy, None)
}

pub fn resolve_dependencies_preferred(
    pid: &PackageId,
    strategy: VersionStrategy,
    preferred_source: Option<RepoSource>,
) -> SpmResult<ResolvedGraph> {
    let packages = solver::resolve_with_solver(
        &pid.name, pid.format.clone(), strategy, preferred_source)?;
    Ok(ResolvedGraph {
        topological_order: packages,
        cycles: Vec::new(),
        unresolved: Vec::new(),
        metadata_conflicts: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolved_graph_default() {
        let g = ResolvedGraph::default();
        assert!(g.topological_order.is_empty());
        assert!(g.cycles.is_empty());
        assert!(g.unresolved.is_empty());
        assert!(g.metadata_conflicts.is_empty());
    }

    #[test]
    fn test_metadata_conflict_types() {
        let mc1 = MetadataConflict {
            package: "nginx".into(),
            conflicts_with: "apache".into(),
            conflict_type: ConflictType::SystemInstalled,
        };
        let mc2 = MetadataConflict {
            package: "nginx".into(),
            conflicts_with: "lighttpd".into(),
            conflict_type: ConflictType::SameBatch,
        };
        assert_eq!(mc1.conflict_type, ConflictType::SystemInstalled);
        assert_eq!(mc2.conflict_type, ConflictType::SameBatch);
        assert_ne!(mc1.conflict_type, mc2.conflict_type);
    }

    #[test]
    fn test_bump_dep_cache_epoch() {
        bump_dep_cache_epoch();
    }
}
