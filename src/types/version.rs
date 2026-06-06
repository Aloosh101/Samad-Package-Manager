use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    pub epoch: u32,
    pub version: String,
    pub release: String,
}

impl Version {
    pub fn parse(ver: &str) -> Self {
        let (epoch, rest) = if let Some((e, r)) = ver.split_once(':') {
            (e.parse().unwrap_or(0), r)
        } else {
            (0, ver)
        };
        let (version, release) = rest.rsplit_once('-').unwrap_or((rest, ""));
        Self { epoch, version: version.to_string(), release: release.to_string() }
    }

    pub fn compare(a: &str, b: &str) -> std::cmp::Ordering {
        let va = Self::parse(a);
        let vb = Self::parse(b);
        va.cmp(&vb)
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.epoch.cmp(&other.epoch)
            .then_with(|| rpmvercmp(&self.version, &other.version))
            .then_with(|| rpmvercmp(&self.release, &other.release))
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.epoch > 0 {
            write!(f, "{}:{}-{}", self.epoch, self.version, self.release)
        } else if self.release.is_empty() {
            write!(f, "{}", self.version)
        } else {
            write!(f, "{}-{}", self.version, self.release)
        }
    }
}

/// Strategy for choosing between versions during dependency resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VersionStrategy {
    /// Prefer packages from highest-priority repos (lowest priority number).
    /// This is the default and gives the most "stable" result.
    #[default]
    PreferStable,
    /// Prefer the highest version number regardless of repo priority.
    /// Useful when you want the latest features (bleeding edge).
    PreferNewest,
}

/// RPM version comparison algorithm.
pub fn rpmvercmp(a: &str, b: &str) -> std::cmp::Ordering {
    let a = a.trim();
    let b = b.trim();
    if a == b { return std::cmp::Ordering::Equal; }

    let (mut ai, mut bi) = (0usize, 0usize);
    let a_bytes = a.as_bytes();
    let b_bytes = b.as_bytes();

    while ai < a_bytes.len() || bi < b_bytes.len() {
        if ai < a_bytes.len() && a_bytes[ai] == b'~' {
            if bi >= b_bytes.len() || b_bytes[bi] != b'~' { return std::cmp::Ordering::Less; }
            ai += 1; bi += 1;
            continue;
        }
        if bi < b_bytes.len() && b_bytes[bi] == b'~' { return std::cmp::Ordering::Greater; }

        while ai < a_bytes.len() && !a_bytes[ai].is_ascii_alphanumeric() { ai += 1; }
        while bi < b_bytes.len() && !b_bytes[bi].is_ascii_alphanumeric() { bi += 1; }

        if ai >= a_bytes.len() || bi >= b_bytes.len() { break; }

        if a_bytes[ai].is_ascii_digit() && b_bytes[bi].is_ascii_digit() {
            while ai + 1 < a_bytes.len() && a_bytes[ai] == b'0' && a_bytes[ai + 1].is_ascii_digit() { ai += 1; }
            while bi + 1 < b_bytes.len() && b_bytes[bi] == b'0' && b_bytes[bi + 1].is_ascii_digit() { bi += 1; }

            let anum_start = ai;
            while ai < a_bytes.len() && a_bytes[ai].is_ascii_digit() { ai += 1; }
            let anum = &a_bytes[anum_start..ai];

            let bnum_start = bi;
            while bi < b_bytes.len() && b_bytes[bi].is_ascii_digit() { bi += 1; }
            let bnum = &b_bytes[bnum_start..bi];

            match anum.len().cmp(&bnum.len()) {
                std::cmp::Ordering::Equal => {}
                other => return other,
            }
            for (&ac, &bc) in anum.iter().zip(bnum.iter()) {
                if ac != bc { return ac.cmp(&bc); }
            }
            continue;
        }

        if a_bytes[ai].is_ascii_alphabetic() && b_bytes[bi].is_ascii_alphabetic() {
            let a_start = ai;
            while ai < a_bytes.len() && a_bytes[ai].is_ascii_alphabetic() { ai += 1; }
            let b_start = bi;
            while bi < b_bytes.len() && b_bytes[bi].is_ascii_alphabetic() { bi += 1; }
            match a_bytes[a_start..ai].cmp(&b_bytes[b_start..bi]) {
                std::cmp::Ordering::Equal => {}
                other => return other,
            }
            continue;
        }

        if a_bytes[ai].is_ascii_alphabetic() { return std::cmp::Ordering::Greater; }
        if b_bytes[bi].is_ascii_alphabetic() { return std::cmp::Ordering::Less; }

        ai += 1;
        bi += 1;
    }

    (a_bytes.len() - ai).cmp(&(b_bytes.len() - bi))
}
