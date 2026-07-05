use std::collections::HashSet;

use pubgrub::{
    Dependencies, DependencyConstraints, DependencyProvider,
    PubGrubError, Ranges,
};

use crate::error::{SpmError, SpmResult};
use crate::index::{SonameIndex, SonameProvider};
use crate::types::{PackageFormat, PackageId, RepoSource, Version, VersionStrategy};

type P = String;
type V = Version;
type VS = Ranges<V>;

/// Extract the package name from a dependency string, stripping version constraints.
/// "libssl.so.3()(64bit)" -> "libssl.so.3"
/// "libc6 (>= 2.31)" -> "libc6"
/// "libc6 >= 2.31" -> "libc6"
fn extract_soname_name(raw: &str) -> &str {
    raw.split_once(' ')
        .map(|(n, _)| n.trim())
        .unwrap_or(raw)
        .split('(')
        .next()
        .unwrap_or(raw)
        .trim()
}

pub fn extract_dep_name(raw: &str) -> &str {
    raw.split_once(' ')
        .map(|(n, _)| n.trim())
        .unwrap_or(raw)
        .split('(')
        .next()
        .unwrap_or(raw)
        .trim()
}

/// Parse a single dependency string like `"libc6 >= 2.31"` or `"libc6 (>= 2.31)"`
/// into a package name and version range.
fn dep_string_to_range(raw: &str) -> (String, VS) {
    let raw = raw.trim();

    // Try DEB format: "name (op ver)" e.g. "libc6 (>= 2.31)"
    if let Some((name_part, rest)) = raw.split_once('(') {
        let name = name_part.trim().to_string();
        let inner = rest.trim_end_matches(')').trim();
        let (op_str, ver_str) = split_op_ver(inner);
        let range = op_str_to_range(&op_str, &ver_str);
        return (name, range);
    }

    // Try RPM format: "name op ver" e.g. "libc6 >= 2.31"
    for op_token in &[">=", "<=", ">>", "<<", ">", "<", "="] {
        if let Some((name_part, ver_part)) = raw.split_once(op_token) {
            let name = name_part.trim().to_string();
            let ver_str = ver_part.trim();
            let op_str = op_token.to_string();
        let range = op_str_to_range(&op_str, ver_str);
            return (name, range);
        }
    }

    // Plain package name, no constraint
    let name = extract_soname_name(raw).to_string();
    (name, VS::full())
}

fn split_op_ver(s: &str) -> (String, String) {
    let s = s.trim();
    for op in &[">=", "<=", ">>", "<<", ">", "<", "="] {
        if let Some((_, ver_part)) = s.split_once(op) {
            return (op.to_string(), ver_part.trim().to_string());
        }
    }
    (String::new(), s.to_string())
}

fn op_str_to_range(op: &str, ver: &str) -> VS {
    let v = Version::parse(ver);
    match op {
        ">=" | ">>" => VS::higher_than(v),
        ">" => VS::strictly_higher_than(v),
        "<=" | "<<" => VS::lower_than(v),
        "<" => VS::strictly_lower_than(v),
        "=" => VS::singleton(v),
        _ => VS::full(),
    }
}

/// Map a PubGrub solution (package-name + capability entries) to a deduplicated
/// list of real packages to install. Capabilities (*.so, file paths, rpmlib)
/// are resolved to their best-priority provider.
fn solution_to_packages(
    solution: Vec<(String, Version)>,
    index: &SonameIndex,
    format: &PackageFormat,
) -> Vec<PackageId> {
    let mut pkgs = Vec::new();
    let mut seen = HashSet::new();

    for (name, ver) in solution {
        if name.starts_with("__or_") || name.starts_with("rpmlib(") {
            continue;
        }

        let is_capability = name.contains(".so") || name.starts_with('/');

        if is_capability {
            if let Some(providers) = index.get_providers(&name) {
                if let Some(best) = providers.iter().min_by_key(|p| p.priority) {
                    if seen.insert(best.pkg.clone()) {
                        pkgs.push(PackageId::new_v(
                            &best.pkg,
                            format.clone(),
                            &best.version,
                        ));
                    }
                }
            }
        } else if seen.insert(name.clone()) {
            pkgs.push(PackageId::new_v(&name, format.clone(), &ver.to_string()));
        }
    }

    pkgs
}

pub struct SpmDepProvider {
    index: SonameIndex,
    strategy: VersionStrategy,
    preferred_source: Option<RepoSource>,
}

impl SpmDepProvider {
    pub fn new(index: SonameIndex, strategy: VersionStrategy) -> Self {
        Self { index, strategy, preferred_source: None }
    }

    pub fn with_preferred_source(mut self, source: RepoSource) -> Self {
        self.preferred_source = Some(source);
        self
    }

    fn get_providers(&self, package: &str) -> Vec<SonameProvider> {
        self.index
            .get_providers(package)
            .map(|p| p.to_vec())
            .unwrap_or_default()
    }

    fn get_requires(&self, package: &str) -> Vec<String> {
        self.index
            .get_requires(package)
            .map(|r| r.to_vec())
            .unwrap_or_default()
    }

    fn best_version(&self, package: &str) -> Option<Version> {
        let providers = self.get_providers(package);
        if providers.is_empty() {
            return None;
        }
        // Separate into preferred-source and fallback groups
        let preferred_source = self.preferred_source.clone();
        let (mut preferred, mut fallback): (Vec<_>, Vec<_>) = providers
            .iter()
            .partition(|p| match preferred_source.as_ref() {
                Some(s) => p.source == *s,
                None => false,
            });

        match self.strategy {
            VersionStrategy::PreferNewest => {
                // Highest version first; preferred source wins ties
                let best_fallback = fallback.iter().max_by_key(|p| Version::parse(&p.version));
                let best_preferred = preferred.iter().max_by_key(|p| Version::parse(&p.version));
                match (best_preferred, best_fallback) {
                    (Some(a), Some(b)) => {
                        let va = Version::parse(&a.version);
                        let vb = Version::parse(&b.version);
                        if va >= vb { Some(va) } else { Some(vb) }
                    }
                    (Some(a), None) => Some(Version::parse(&a.version)),
                    (None, Some(b)) => Some(Version::parse(&b.version)),
                    (None, None) => None,
                }
            }
            VersionStrategy::PreferStable => {
                // Preferred source first, then highest-priority repo (lowest number)
                preferred.sort_by_key(|p| p.priority);
                fallback.sort_by_key(|p| p.priority);
                let chosen = preferred.first().or_else(|| fallback.first());
                chosen.map(|p| Version::parse(&p.version))
            }
        }
    }
}

impl DependencyProvider for SpmDepProvider {
    type P = P;
    type V = V;
    type VS = VS;
    type M = String;
    type Err = SpmError;
    type Priority = usize;

    fn choose_version(
        &self,
        _package: &P,
        range: &VS,
    ) -> Result<Option<V>, Self::Err> {
        let best = self.best_version(_package);
        match best {
            Some(v) if range.contains(&v) => Ok(Some(v)),
            _ => Ok(None),
        }
    }

    fn prioritize(
        &self,
        _package: &P,
        range: &VS,
        _conflicts_counts: &pubgrub::PackageResolutionStatistics,
    ) -> Self::Priority {
        match self.best_version(_package) {
            Some(ref v) if range.contains(v) => 1,
            _ => usize::MAX,
        }
    }

    fn get_dependencies(
        &self,
        package: &P,
        _version: &V,
    ) -> Result<Dependencies<P, VS, Self::M>, Self::Err> {
        let requires = self.get_requires(package);
        let constraints: DependencyConstraints<P, VS> = requires
            .iter()
            .map(|r| dep_string_to_range(r))
            .filter(|(name, _)| !name.is_empty())
            .collect();
        Ok(Dependencies::Available(constraints))
    }
}

pub fn resolve_with_solver(
    root_package: &str,
    root_format: PackageFormat,
    strategy: VersionStrategy,
    preferred_source: Option<RepoSource>,
) -> SpmResult<Vec<PackageId>> {
    let index = SonameIndex::load()?;
    let mut provider = SpmDepProvider::new(index.clone(), strategy);
    if let Some(src) = preferred_source {
        provider = provider.with_preferred_source(src);
    }

    let root_version = Version {
        epoch: 0,
        version: "0".into(),
        release: "0".into(),
    };

    match pubgrub::resolve(&provider, root_package.to_string(), root_version) {
        Ok(solution) => {
            let packages = solution_to_packages(
                solution.into_iter().collect(),
                &index,
                &root_format,
            );
            Ok(packages)
        }
        Err(PubGrubError::NoSolution(_)) => {
            tracing::info!(
                "PubGrub failed for '{}' ({:?}), falling back to native backend resolver",
                root_package,
                root_format,
            );
            fallback_to_native_resolver(root_package, &root_format)
        }
        Err(e) => Err(SpmError::resolution_failed(format!(
            "Solver error for {}: {:?}",
            root_package, e
        ))),
    }
}

/// Fallback: when PubGrub cannot find a solution, try reading from the
/// SONAME index directly.  This is still a pure-Rust path.
fn fallback_to_native_resolver(
    root_package: &str,
    _format: &PackageFormat,
) -> SpmResult<Vec<PackageId>> {
    let index = SonameIndex::load()?;
    let mut pkgs = Vec::new();
    let mut seen = HashSet::new();
    seen.insert(root_package.to_string());
    pkgs.push(PackageId::new(root_package, _format.clone()));

    // Walk requires from the SONAME index to build the closure
    if let Some(requires) = index.get_requires(root_package) {
        for req in requires {
            let name = extract_soname_name(req);
            if !name.is_empty() && seen.insert(name.to_string()) {
                let fmt = _format.clone();
                pkgs.push(PackageId::new(name, fmt));
            }
        }
    }

    Ok(pkgs)
}
