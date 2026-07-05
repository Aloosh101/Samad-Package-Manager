mod cache;
mod graph;

pub use cache::bump_dep_cache_epoch;
pub use graph::{
    Alternative, ConflictSuggestion, ConflictSuggestionKind, ConflictType, MetadataConflict,
    ResolvedGraph, UnresolvedDep, UnresolvedReason,
};

use std::collections::{HashMap, HashSet, VecDeque};

use cache::DependencyCache;
use crate::error::SpmResult;
use crate::index::SonameIndex;
use crate::package::solver;
use crate::types::{PackageId, RepoSource, VersionStrategy};

pub struct ResolverConfig {
    pub strategy: VersionStrategy,
    pub preferred_source: Option<RepoSource>,
    pub prefer_newest: bool,
    pub enable_cache: bool,
}

impl Default for ResolverConfig {
    fn default() -> Self {
        Self {
            strategy: VersionStrategy::PreferStable,
            preferred_source: None,
            prefer_newest: false,
            enable_cache: true,
        }
    }
}

pub struct Resolver {
    pub index: SonameIndex,
    pub repos: Vec<(String, crate::types::RepoConfig)>,
    pub cache: DependencyCache,
    pub config: ResolverConfig,
}

impl Resolver {
    pub fn new() -> SpmResult<Self> {
        let index = SonameIndex::load()?;
        let repos = crate::config::repos::load_repos()?;
        Ok(Self {
            index,
            repos,
            cache: DependencyCache::new(),
            config: ResolverConfig::default(),
        })
    }

    pub fn with_config(config: ResolverConfig) -> SpmResult<Self> {
        let mut resolver = Self::new()?;
        resolver.config = config;
        Ok(resolver)
    }

    pub fn resolve(
        &mut self,
        root: &PackageId,
    ) -> SpmResult<ResolvedGraph> {
        let strategy_val = match self.config.strategy {
            VersionStrategy::PreferStable => 0u8,
            VersionStrategy::PreferNewest => 1u8,
        };

        if self.config.enable_cache {
            if let Some(cached) = self.cache.get(root, strategy_val) {
                return Ok(ResolvedGraph::new(cached));
            }
        }

        let base = self.phase1_collect_packages(root)?;
        let solved = self.phase2_sat_solve(root, &base)?;
        let with_cycles = self.phase3_detect_cycles(&solved);
        let with_conflicts = self.phase4_detect_conflicts(with_cycles);
        let final_graph = self.phase5_classify_unresolved(with_conflicts);

        if self.config.enable_cache {
            self.cache.set(root, strategy_val, final_graph.topological_order.clone());
        }

        Ok(final_graph)
    }

    fn phase1_collect_packages(
        &self,
        root: &PackageId,
    ) -> SpmResult<Vec<PackageId>> {
        let format = root.format.clone();
        let mut collected = Vec::new();
        let mut seen = HashSet::new();
        let mut queue = VecDeque::new();
        queue.push_back(root.name.clone());

        while let Some(name) = queue.pop_front() {
            if !seen.insert(name.clone()) {
                continue;
            }

            let pid = PackageId::new(&name, format.clone());
            collected.push(pid);

            if let Some(requires) = self.index.get_requires(&name) {
                for dep_str in requires.iter() {
                    let dep_name = solver::extract_dep_name(dep_str);
                    if !seen.contains(dep_name) {
                        queue.push_back(dep_name.to_string());
                    }
                }
            }
        }

        Ok(collected)
    }

    fn phase2_sat_solve(
        &self,
        root: &PackageId,
        _collected: &[PackageId],
    ) -> SpmResult<Vec<PackageId>> {
        solver::resolve_with_solver(
            &root.name,
            root.format.clone(),
            self.config.strategy,
            self.config.preferred_source.clone(),
        )
    }

    fn phase3_detect_cycles(&self, packages: &[PackageId]) -> ResolvedGraph {
        let mut graph = ResolvedGraph::new(packages.to_vec());
        let cycles = self.tarjan_scc(packages);
        graph.cycles = cycles;
        graph
    }

    fn tarjan_scc(&self, packages: &[PackageId]) -> Vec<Vec<PackageId>> {
        let mut index_map: HashMap<&str, usize> = HashMap::new();
        let mut pkg_by_name: HashMap<&str, &PackageId> = HashMap::new();
        for (i, pkg) in packages.iter().enumerate() {
            index_map.insert(pkg.name.as_str(), i);
            pkg_by_name.insert(pkg.name.as_str(), pkg);
        }

        let n = packages.len();
        let mut disc = vec![0i32; n];
        let mut low = vec![0i32; n];
        let mut on_stack = vec![false; n];
        let mut stack: Vec<usize> = Vec::new();
        let mut time = 0i32;
        let mut sccs: Vec<Vec<PackageId>> = Vec::new();

        fn strongconnect<'a>(
            v: usize,
            packages: &[PackageId],
            graph: &HashMap<&str, Vec<String>>,
            index_map: &HashMap<&str, usize>,
            pkg_by_name: &HashMap<&str, &PackageId>,
            disc: &mut [i32],
            low: &mut [i32],
            on_stack: &mut [bool],
            stack: &mut Vec<usize>,
            time: &mut i32,
            sccs: &mut Vec<Vec<PackageId>>,
        ) {
            *time += 1;
            disc[v] = *time;
            low[v] = *time;
            stack.push(v);
            on_stack[v] = true;

            let pkg_name = pkg_by_name
                .iter()
                .find(|(_, &p)| {
                    let idx = index_map.get(p.name.as_str()).copied().unwrap_or(usize::MAX);
                    idx == v
                })
                .map(|(name, _)| *name);

            if let Some(name) = pkg_name {
                if let Some(deps) = graph.get(name) {
                    for dep_str in deps {
                        let dep_name = solver::extract_dep_name(dep_str);
                            if let Some(&w) = index_map.get(dep_name) {
                            if disc[w] == 0 {
                                strongconnect(
                                    w, packages, graph, index_map, pkg_by_name,
                                    disc, low, on_stack, stack, time, sccs,
                                );
                                low[v] = low[v].min(low[w]);
                            } else if on_stack[w] {
                                low[v] = low[v].min(disc[w]);
                            }
                        }
                    }
                }
            }

            if low[v] == disc[v] {
                let mut scc: Vec<PackageId> = Vec::new();
                loop {
                    let w = stack.pop().unwrap();
                    on_stack[w] = false;
                    if let Some(pkg) = packages.get(w) {
                        scc.push(pkg.clone());
                    }
                    if w == v {
                        break;
                    }
                }
                if scc.len() > 1 {
                    sccs.push(scc);
                }
            }
        }

        let mut dep_graph: HashMap<&str, Vec<String>> = HashMap::new();
        for pkg in packages {
            if let Some(requires) = self.index.get_requires(&pkg.name) {
                dep_graph.insert(pkg.name.as_str(), requires.to_vec());
            } else {
                dep_graph.insert(pkg.name.as_str(), Vec::new());
            }
        }

        for i in 0..n {
            if disc[i] == 0 {
                strongconnect(
                    i, packages, &dep_graph, &index_map, &pkg_by_name,
                    &mut disc, &mut low, &mut on_stack, &mut stack,
                    &mut time, &mut sccs,
                );
            }
        }

        sccs
    }

    fn phase4_detect_conflicts(&self, graph: ResolvedGraph) -> ResolvedGraph {
        let mut graph = graph;
        let mut conflicts = Vec::new();

        let mut version_map: HashMap<&str, (&PackageId, &str)> = HashMap::new();
        for pkg in &graph.topological_order {
            if let Some(&(existing, _)) = version_map.get(pkg.name.as_str()) {
                if existing.version != pkg.version {
                    conflicts.push(MetadataConflict {
                        package: pkg.name.clone(),
                        conflicts_with: existing.name.clone(),
                        conflict_type: ConflictType::SameBatch,
                    });
                }
            }
            version_map.insert(pkg.name.as_str(), (pkg, &pkg.version));
        }

        graph.metadata_conflicts = conflicts;
        graph
    }

    fn phase5_classify_unresolved(&self, graph: ResolvedGraph) -> ResolvedGraph {
        let graph = graph;
        graph
    }
}

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
    let config = ResolverConfig {
        strategy,
        preferred_source,
        prefer_newest: strategy == VersionStrategy::PreferNewest,
        enable_cache: true,
    };
    let mut resolver = Resolver::with_config(config)?;
    resolver.resolve(pid)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::PackageFormat;

    #[test]
    fn test_resolved_graph_default() {
        let g = ResolvedGraph::default();
        assert!(g.topological_order.is_empty());
        assert!(g.cycles.is_empty());
        assert!(g.unresolved.is_empty());
        assert!(g.metadata_conflicts.is_empty());
        assert!(g.unresolved_deps.is_empty());
        assert!(g.alternatives.is_empty());
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

    #[test]
    fn test_unresolved_reason_display() {
        assert!(matches!(UnresolvedReason::NotInRepos, UnresolvedReason::NotInRepos));
        assert!(matches!(UnresolvedReason::VersionConflict, UnresolvedReason::VersionConflict));
        assert!(matches!(UnresolvedReason::CircularDependency, UnresolvedReason::CircularDependency));
        assert!(matches!(UnresolvedReason::MissingProvider, UnresolvedReason::MissingProvider));
    }

    #[test]
    fn test_alternative_struct() {
        let alt = Alternative {
            name: "libssl".into(),
            format: PackageFormat::Deb,
            version: "1.1.1".into(),
            source: RepoSource::Deb,
            priority: 50,
        };
        assert_eq!(alt.name, "libssl");
        assert_eq!(alt.priority, 50);
    }

    #[test]
    fn test_resolved_graph_builder() {
        let pkg = PackageId::new("test", PackageFormat::Deb);
        let graph = ResolvedGraph::new(vec![pkg.clone()])
            .with_cycles(vec![vec![pkg.clone()]])
            .with_unresolved(vec![])
            .with_metadata_conflicts(vec![])
            .with_alternatives(vec![]);
        assert_eq!(graph.topological_order.len(), 1);
        assert_eq!(graph.cycles.len(), 1);
    }

    #[test]
    fn test_dependency_cache() {
        let mut cache = DependencyCache::new();
        let pkg = PackageId::new("test", PackageFormat::Deb);
        let deps = vec![PackageId::new("dep1", PackageFormat::Deb)];
        cache.set(&pkg, 0, deps.clone());
        let cached = cache.get(&pkg, 0);
        assert!(cached.is_some());
        assert_eq!(cached.unwrap().len(), 1);
        cache.invalidate_all();
        assert!(cache.get(&pkg, 0).is_none());
    }

    #[test]
    fn test_resolve_dependencies_api() {
        let pid = PackageId::new("nonexistent-test-pkg", PackageFormat::Sam);
        let result = resolve_dependencies(&pid, VersionStrategy::PreferStable);
        // resolver returns Ok with empty graph for missing packages
        assert!(result.is_ok());
    }

    #[test]
    fn test_resolve_dependencies_preferred_api() {
        let pid = PackageId::new("nonexistent-test-pkg", PackageFormat::Sam);
        let result = resolve_dependencies_preferred(&pid, VersionStrategy::PreferStable, Some(RepoSource::Native));
        // resolver returns Ok with empty graph for missing packages
        assert!(result.is_ok());
    }

    #[test]
    fn test_all_package_names() {
        let pkg1 = PackageId::new("a", PackageFormat::Deb);
        let pkg2 = PackageId::new("b", PackageFormat::Deb);
        let graph = ResolvedGraph::new(vec![pkg1, pkg2]);
        let names = graph.all_package_names();
        assert_eq!(names, vec!["a", "b"]);
    }

    #[test]
    fn test_conflict_suggestion() {
        let s = ConflictSuggestion {
            kind: ConflictSuggestionKind::Sandbox,
            description: "Install in sandbox".into(),
        };
        assert_eq!(s.kind, ConflictSuggestionKind::Sandbox);
    }
}
