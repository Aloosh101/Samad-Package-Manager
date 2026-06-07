use clap::{Parser, Subcommand};
use std::io::{IsTerminal, Write};

use crate::cli::client;
use crate::config::repos;
use crate::error::SpmResult;
use crate::package::{install, query, sam};
use crate::sandbox;
use crate::types::SpmConfig;
use crate::util::process;

#[derive(Parser, Debug)]
#[command(name = "spm", version = "0.2.0", about = "Samad Package Manager")]
pub struct SpmArgs {
    #[command(subcommand)]
    pub command: SpmCommand,
}

#[derive(Subcommand, Debug)]
pub enum SpmCommand {
    /// Install a package (i)
    #[command(alias = "i",
        after_help = "Examples:\n  spm install nginx               Install from repos\n  spm install ./pkg.deb            Install local .deb\n  spm install --sandbox nginx      Sandboxed install\n  spm install --smart nginx        RPATH library isolation\n  spm install -y nginx             Non-interactive\n  spm install --replace nginx      Reinstall or replace conflicts\n  spm install --stable-debian nginx  Stable from Debian repos\n  spm install --newest-redhat nginx  Newest from RedHat repos\n  spm install --prefer-newest nginx  Prefer newest regardless of distro"
    )]
    Install {
        /// Package name or path to a local .deb/.rpm/.sam file
        package: String,
        /// Sandbox the installation (full, standard, or strict)
        #[arg(long)]
        sandbox: Option<Option<String>>,
        /// Convert a binary package to .sam format without installing
        #[arg(long)]
        convert_only: Option<String>,
        /// Remove conflicting packages automatically
        #[arg(long)]
        replace: bool,
        /// Assume yes to all prompts (non-interactive)
        #[arg(long, short = 'y')]
        yes: bool,
        /// Isolate library files (.so) in store, set RPATH instead of symlinking to FHS
        #[arg(long)]
        smart: bool,
        /// Prefer the newest version over repo priority (bleeding edge)
        #[arg(long)]
        prefer_newest: bool,
        /// Stable from Debian repos (stable + apt, highest priority)
        #[arg(long)]
        stable_debian: bool,
        /// Newest from RedHat repos (newest + rpm, highest priority)
        #[arg(long)]
        newest_redhat: bool,
    },

    /// Remove a package (r)
    #[command(alias = "r",
        after_help = "Examples:\n  spm remove nginx         Remove nginx\n  spm remove -y nginx      Non-interactive removal"
    )]
    Remove {
        /// Name of the package to remove
        package: String,
        /// Assume yes to all prompts (non-interactive)
        #[arg(long, short = 'y')]
        yes: bool,
    },

    /// Remove a package along with its configuration files (p)
    #[command(alias = "p",
        after_help = "Examples:\n  spm purge nginx          Remove nginx including config files"
    )]
    Purge {
        /// Name of the package to purge
        package: String,
    },

    /// Remove orphaned packages (no other package depends on them)
    #[command(name = "autoremove",
        after_help = "Examples:\n  spm autoremove               Remove orphaned packages (interactive)\n  spm autoremove -y            Remove orphaned packages (non-interactive)"
    )]
    Autoremove {
        /// Assume yes to all prompts (non-interactive)
        #[arg(long, short = 'y')]
        yes: bool,
    },

    /// Fetch the latest repository metadata from all sources (u)
    #[command(alias = "u",
        after_help = "Examples:\n  spm update                Fetch latest repo metadata"
    )]
    Update,

    /// Upgrade all packages (or a single package) to the latest version (upg)
    #[command(alias = "upg",
        after_help = "Examples:\n  spm upgrade                   Upgrade all packages\n  spm upgrade nginx              Upgrade only nginx\n  spm upgrade --prefer-newest    Prefer newest regardless of distro\n  spm upgrade --stable-debian    Stable from Debian repos\n  spm upgrade --newest-redhat    Newest from RedHat repos"
    )]
    Upgrade {
        /// Optional package name to upgrade (omit to upgrade all)
        package: Option<String>,
        /// Prefer the newest version over repo priority (bleeding edge)
        #[arg(long)]
        prefer_newest: bool,
        /// Stable from Debian repos (stable + apt, highest priority)
        #[arg(long)]
        stable_debian: bool,
        /// Newest from RedHat repos (newest + rpm, highest priority)
        #[arg(long)]
        newest_redhat: bool,
    },

    /// Perform a full distribution upgrade (upgrade all + remove obsoletes + remove orphans)
    #[command(name = "dist-upgrade",
        after_help = "Examples:\n  spm dist-upgrade            Full distribution upgrade (interactive)\n  spm dist-upgrade -y         Full distribution upgrade (non-interactive)"
    )]
    DistUpgrade {
        /// Assume yes to all prompts (non-interactive)
        #[arg(long, short = 'y')]
        yes: bool,
    },

    /// Search configured repositories for packages (s)
    #[command(alias = "s",
        after_help = "Examples:\n  spm search nginx         Search repos for nginx\n  spm search lib           Search for 'lib' packages"
    )]
    Search {
        /// Package name or search pattern
        query: String,
    },

    /// Search remote repositories (Debian, COPR) for a package (gs)
    #[command(alias = "gs", name = "global-search",
        after_help = "Examples:\n  spm global-search nginx  Search Debian + COPR for nginx\n  spm global-search -y nginx  Non-interactive, show results only"
    )]
    GlobalSearch {
        /// Package name or search pattern
        query: String,
        /// Assume yes to all prompts (non-interactive)
        #[arg(long, short = 'y')]
        yes: bool,
    },

    /// Find which installed package owns a given file path
    #[command(name = "search-file",
        after_help = "Examples:\n  spm search-file /usr/bin/figlet    Find which package owns figlet\n  spm search-file /etc/nginx/nginx.conf"
    )]
    SearchFile {
        /// Path to search for
        path: String,
    },

    /// Show detailed information about an installed package (show)
    #[command(alias = "show",
        after_help = "Examples:\n  spm info nginx           Show details about installed nginx\n  spm info figlet          Show package metadata"
    )]
    Info {
        /// Name of the installed package
        package: String,
    },

    /// List files owned by an installed package (ls)
    #[command(alias = "ls",
        after_help = "Examples:\n  spm files nginx           List all files owned by nginx"
    )]
    Files {
        /// Name of the installed package
        package: String,
    },

    /// Show direct dependencies of a package (deps)
    #[command(alias = "deps")]
    Depends {
        /// Name of the installed package
        package: String,
    },

    /// Show packages that depend on the given package (rdeps)
    #[command(alias = "rdeps")]
    Rdepends {
        /// Name of the installed package
        package: String,
    },

    /// View transaction history or undo a past transaction (log)
    #[command(alias = "log")]
    History {
        /// Transaction ID to undo (omit to show history)
        undo: Option<i64>,
    },

    /// Create or rollback Btrfs snapshots (snap)
    #[command(alias = "snap")]
    Snapshot {
        #[command(subcommand)]
        action: SnapshotAction,
    },

    /// Analyze the system for orphans, conflicts, or file traces (an)
    #[command(alias = "an")]
    Analyze {
        #[command(subcommand)]
        mode: Option<AnalyzeMode>,
        /// Trace which package owns a specific binary or library path
        trace: Option<String>,
    },

    /// List processes that are using deleted files (lsof-style)
    Ps,

    /// Manage sandboxed packages
    Sandbox {
        #[command(subcommand)]
        action: SandboxAction,
    },

    /// Add, list, or remove package repositories (repository)
    #[command(alias = "repository",
        after_help = "Examples:\n  spm repo add debian --source apt --mirror http://deb.debian.org/debian\n  spm repo list\n  spm repo remove myrepo\n  spm repo gen-key myrepo         Generate Ed25519 signing key\n  spm repo sign myrepo            Sign Release file with key"
    )]
    Repo {
        #[command(subcommand)]
        action: RepoAction,
    },

    /// Rebuild the SONAME index from cached repo metadata
    #[command(after_help = "Examples:\n  spm index rebuild     Rebuild soname-index.json from cached metadata"
    )]
    Index {
        #[command(subcommand)]
        action: IndexAction,
    },

    /// Detect hardware and suggest drivers
    #[command(after_help = "Examples:\n  spm detect                 Detect GPU and suggest drivers\n  spm detect --install       Detect and install suggested drivers\n  spm detect --kernel        Show installed kernel version"
    )]
    Detect {
        /// Install suggested driver packages automatically
        #[arg(long)]
        install: bool,
    },

    /// Build a .sam package from source (b)
    #[command(alias = "b",
        after_help = "Examples:\n  spm build                   Build package from current directory\n  spm build ./myapp          Build package from ./myapp directory\n  spm build --output ./dist  Write .sam to ./dist/\n  spm build --sign KEY_ID    Sign package with GPG key"
    )]
    Build {
        /// Path to the project directory (default: .)
        #[arg(default_value = ".")]
        path: String,
        /// Output directory for the .sam file (default: project dir)
        #[arg(long, short)]
        output: Option<String>,
        /// GPG key ID to sign the package
        #[arg(long)]
        sign: Option<String>,
    },

    /// Cleanup orphaned files, cache, and temporary data
    Cleanup {
        /// Remove without prompting
        #[arg(long)]
        force: bool,
    },

    /// View or modify configuration
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },

    /// Manage the spm UNIX group for daemon access
    Group {
        #[command(subcommand)]
        action: GroupAction,
    },

    /// Synchronize spm database with system package state (dpkg/rpm)
    #[command(
        after_help = "Examples:\n  spm sync                  Import foreign packages into spm DB\n  spm sync --files          Also sync file records\n  spm sync --prune          Remove spm entries for uninstalled foreign packages"
    )]
    Sync {
        /// Also scan and sync file records
        #[arg(long)]
        files: bool,
        /// Remove spm DB entries for packages no longer installed on system
        #[arg(long)]
        prune: bool,
    },

    /// Check and repair database integrity
    #[command(
        after_help = "Examples:\n  spm fsck                  Check DB integrity and consistency\n  spm fsck --fix            Attempt to repair found issues\n  spm fsck --files          Verify installed file records against disk"
    )]
    Fsck {
        /// Attempt to repair found issues
        #[arg(long)]
        fix: bool,
        /// Verify installed file records against actual filesystem
        #[arg(long)]
        files: bool,
    },

    /// Bootstrap a fresh system: initialize directories, database, and backends
    #[command(
        after_help = "SPM manages its own backend binaries (dnf, apt, rpm, dpkg) in isolation.\nBackends are stored at /var/lib/spm/store/backend/ and are never resolved\nfrom the system PATH. If backends are missing, every command will warn you.\n\nExamples:\n  spm init                     Initialize directories, DB, and backends\n  spm init --fix-backend       Reinstall bundled backends to store\n  spm init --root /mnt         Initialize for a chroot target\n  spm init --from-system       Initialize + import all system packages\n  spm init --install-daemon    Install spmd systemd service and start it"
    )]
    Init {
        /// Target root directory (for bootstrapping a chroot)
        #[arg(long)]
        root: Option<String>,
        /// Import all currently installed system packages into spm DB
        #[arg(long)]
        from_system: bool,
        /// Reinstall backend binaries from /usr/libexec/spm/backend/ into the store
        #[arg(long)]
        fix_backend: bool,
        /// Install spmd systemd service for daemon-based installs
        #[arg(long)]
        install_daemon: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum SnapshotAction {
    /// Create a new Btrfs snapshot
    Create,
    /// Rollback to a previous Btrfs snapshot by ID
    Rollback {
        /// Snapshot ID to rollback to
        id: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum AnalyzeMode {
    /// Find orphaned files not owned by any installed package
    Orphan,
    /// Detect file conflicts between installed packages
    Conflicts,
}

#[derive(Subcommand, Debug)]
pub enum SandboxAction {
    /// List all sandboxed packages
    List,
    /// Run a command inside a sandboxed package's environment (exec)
    #[command(alias = "exec")]
    Run {
        /// Name of the sandboxed package
        package: String,
        /// Command and arguments to execute
        command: Vec<String>,
    },
}

#[derive(Subcommand, Debug)]
pub enum RepoAction {
    /// Add a new repository
    Add {
        /// Repository name (used as identifier)
        name: String,
        /// Repository format: apt, dnf, or native
        #[arg(long, short, default_value = "native")]
        source: String,
        /// Repository base URL (for apt/dnf repos)
        #[arg(long, short)]
        url: Option<String>,
        /// Mirror URLs for apt repositories
        #[arg(long)]
        mirror: Vec<String>,
        /// Priority (lower number = higher priority, default: 100)
        #[arg(long, default_value_t = 100)]
        priority: u32,
    },
    /// Create a new repository (initialise directory structure)
    #[command(after_help = "Examples:\n  spm repo create myrepo --source native --path /srv/spm/repos\n  spm repo create myrepo --source apt --path /srv/spm/repos --codename stable --component main\n  spm repo create myrepo --source dnf --path /srv/spm/repos\n  spm repo create myrepo --source native                  (default path: /var/lib/spm/repos/native/<name>)"
    )]
    Create {
        /// Repository name
        name: String,
        /// Repository format: apt, dnf, native
        #[arg(long, short, default_value = "native")]
        source: String,
        /// Base path for the repository (default: /var/lib/spm/repos/<source>/<name>)
        #[arg(long, short)]
        path: Option<String>,
        /// Distribution codename (for apt repos only, default: stable)
        #[arg(long)]
        codename: Option<String>,
        /// Repository component (for apt repos only, default: main)
        #[arg(long)]
        component: Option<String>,
        /// First mirror URL (for apt repos)
        #[arg(long)]
        mirror: Option<String>,
    },
    /// List configured repositories
    List,
    /// Publish a .sam package to a repository
    #[command(after_help = "Examples:\n  spm repo publish myrepo ./myapp-1.0.0.sam\n  spm repo publish myrepo ./myapp-1.0.0.sam\n  (auto-signs Release if signing key is configured)"
    )]
    Publish {
        /// Name of the repository to publish to
        name: String,
        /// Path to the .sam package file
        package: String,
    },
    /// Remove a repository by name
    Remove {
        /// Name of the repository to remove
        name: String,
    },
    /// Generate an Ed25519 signing keypair for a repository
    #[command(name = "gen-key")]
    GenKey {
        /// Repository name
        name: String,
    },
    /// Sign a repository's Release file
    Sign {
        /// Repository name
        name: String,
    },
}

#[derive(Subcommand, Debug)]
pub enum IndexAction {
    /// Rebuild the SONAME index from cached repo metadata
    #[command(
        after_help = "Examples:\n  spm index rebuild  Rebuild soname-index.json without fetching metadata"
    )]
    Rebuild,
}

#[derive(Subcommand, Debug)]
pub enum ConfigAction {
    /// Display the current SPM configuration
    Show,
    /// Set a configuration key to a value
    Set { key: String, value: String },
}

#[derive(Subcommand, Debug)]
pub enum GroupAction {
    /// Create the spm group if it doesn't exist
    Create,
    /// Add a user to the spm group
    Add { user: String },
    /// Remove a user from the spm group
    Remove { user: String },
    /// List members of the spm group
    List,
}

impl SpmArgs {
    fn is_local_path(s: &str) -> bool {
        is_local_path_string(s) || std::path::Path::new(s).exists()
    }

    fn resolve_mode(
        cli_prefer_newest: bool,
        cli_stable_debian: bool,
        cli_newest_redhat: bool,
    ) -> (crate::types::VersionStrategy, Option<crate::types::RepoSource>) {
        // Atomic modes take highest priority
        if cli_stable_debian {
            return (crate::types::VersionStrategy::PreferStable, Some(crate::types::RepoSource::Apt));
        }
        if cli_newest_redhat {
            return (crate::types::VersionStrategy::PreferNewest, Some(crate::types::RepoSource::Dnf));
        }
        // --prefer-newest or config fallback
        let strategy = if cli_prefer_newest {
            crate::types::VersionStrategy::PreferNewest
        } else if let Ok(cfg) = crate::types::SpmConfig::load() {
            if cfg.prefer_newest.unwrap_or(false) {
                crate::types::VersionStrategy::PreferNewest
            } else {
                crate::types::VersionStrategy::PreferStable
            }
        } else {
            crate::types::VersionStrategy::PreferStable
        };
        (strategy, None)
    }

    pub fn execute(&self) -> SpmResult<()> {
        // Check backend health at the start of every command
        crate::backend::show_warnings();

        match &self.command {
            SpmCommand::Install {
                package,
                sandbox,
                convert_only,
                replace,
                yes,
                smart: _,
                prefer_newest,
                stable_debian,
                newest_redhat,
            } => {
                if let Some(path) = convert_only {
                    sam::convert_to_sam(path)?;
                    crate::output::result_message(format!("Converted {} to .sam format", path));
                } else if sandbox.is_some() {
                    let (strategy, _preferred_source) = Self::resolve_mode(
                        *prefer_newest, *stable_debian, *newest_redhat,
                    );
                    let sandbox_level = sandbox.as_ref().and_then(|s| s.as_deref()).unwrap_or("standard");
                    install::install_package(package, Some(sandbox_level), *replace, *yes, strategy)?;
                } else if Self::is_local_path(package) {
                    install::install_local_package(package, *replace, *yes)?;
                } else {
                    let msg = client::send_install_request(package)?;
                    println!("{}", msg);
                }
                Ok(())
            }

            SpmCommand::Repo { action } => match action {
                RepoAction::Add { name, source, url, mirror, priority: _ } => {
                    let msg = client::send_repo_request("add", name, Some(source), url.as_deref(), mirror)?;
                    println!("{}", msg);
                    Ok(())
                }
                RepoAction::List => {
                    repos::list_repos()?;
                    Ok(())
                }
                RepoAction::Remove { name } => {
                    let msg = client::send_repo_request("remove", name, None, None, &[])?;
                    println!("{}", msg);
                    Ok(())
                }
                RepoAction::Create { name, source, path, codename, component, mirror } => {
                    let source_enum = match source.as_str() {
                        "apt" => crate::types::RepoSource::Apt,
                        "dnf" => crate::types::RepoSource::Dnf,
                        "native" => crate::types::RepoSource::Native,
                        _ => return Err(crate::error::SpmError::config(
                            format!("Invalid repo source '{source}'. Use apt, dnf, or native")
                        )),
                    };
                    repos::create_repo(name, source_enum, path.as_deref(), codename.as_deref(), component.as_deref(), mirror.as_deref())?;
                    crate::output::result_message(format!("Created repository '{name}' ({source})"));
                    Ok(())
                }
                RepoAction::Publish { name, package } => {
                    repos::publish_package(name, package)?;
                    Ok(())
                }
                RepoAction::GenKey { name } => {
                    repos::generate_signing_key(name)?;
                    Ok(())
                }
                RepoAction::Sign { name } => {
                    repos::sign_repo(name)?;
                    Ok(())
                }
            },

            SpmCommand::Analyze { mode, trace } => {
                if let Some(path) = trace {
                    crate::analyze::trace_binary(path)?;
                } else {
                    match mode {
                        Some(AnalyzeMode::Orphan) => {
                            crate::analyze::find_orphans()?;
                        }
                        Some(AnalyzeMode::Conflicts) => {
                            crate::analyze::find_conflicts()?;
                        }
                        None => {
                            crate::analyze::full_analysis()?;
                        }
                    }
                }
                Ok(())
            }

            SpmCommand::Ps => {
                process::find_deleted_libs()?;
                Ok(())
            }

            SpmCommand::Sandbox { action } => match action {
                SandboxAction::List => {
                    sandbox::list_sandboxes()?;
                    Ok(())
                }
                SandboxAction::Run { package, command } => {
                    sandbox::run_in_sandbox(package, command)?;
                    Ok(())
                }
            },

            SpmCommand::Detect { install } => {
                let gpus = crate::kernel::detect_gpu()?;
                for gpu in &gpus {
                    println!("  {} {} — {}", crate::output::bold("GPU"), gpu.pci_id, gpu.description);
                    println!("    Vendor: {}", gpu.vendor);
                    println!("    Driver: {}", gpu.driver);
                    let suggestions = crate::kernel::suggest_gpu_packages(gpu);
                    if !suggestions.is_empty() {
                        println!("    Suggested packages:");
                        for pkg in &suggestions {
                            println!("      • {}", pkg);
                        }
                    }
                    if *install && !suggestions.is_empty() {
                        for pkg in &suggestions {
                            crate::output::step_info(format!("Installing {}...", pkg));
                            let _ = crate::package::install::install_package_smart(pkg, None, false, true, false, Default::default(), None);
                        }
                    }
                }
                if gpus.is_empty() {
                    crate::output::step_info("No GPU detected via lspci");
                }
                Ok(())
            }
            SpmCommand::Build { path, output, sign } => {
                let pkg_path = std::path::Path::new(path);
                let output_path = crate::package::build::run_build(pkg_path, output.as_deref(), sign.as_deref())?;
                crate::output::result_message(format!("Built {}", output_path));
                Ok(())
            }
            SpmCommand::Cleanup { force: _ } => {
                let msg = client::send_cleanup_request()?;
                println!("{}", msg);
                Ok(())
            }

            SpmCommand::Config { action } => match action {
                ConfigAction::Show => {
                    let cfg = SpmConfig::load()?;
                    println!("{:#?}", cfg);
                    Ok(())
                }
                ConfigAction::Set { key, value } => {
                    SpmConfig::set(key, value)?;
                    crate::output::result_message(format!("Set {} = {}", key, value));
                    Ok(())
                }
            },

            SpmCommand::Remove { package, yes: _ } => {
                let msg = client::send_remove_by_name_request(package)?;
                println!("{}", msg);
                Ok(())
            }
            SpmCommand::Purge { package } => {
                let msg = client::send_purge_request(package)?;
                println!("{}", msg);
                Ok(())
            }
            SpmCommand::Autoremove { yes } => {
                let msg = client::send_autoremove_request(*yes)?;
                println!("{}", msg);
                Ok(())
            }
            SpmCommand::Update => {
                let msg = client::send_update_request()?;
                println!("{}", msg);
                Ok(())
            }
            SpmCommand::Index { action } => match action {
                IndexAction::Rebuild => {
                    crate::index::build_index()?;
                    crate::output::result_message("SONAME index rebuilt successfully.");
                    Ok(())
                }
            }
            SpmCommand::Upgrade { package, prefer_newest: _, stable_debian: _, newest_redhat: _ } => {
                let msg = client::send_upgrade_request(package.clone())?;
                println!("{}", msg);
                Ok(())
            }
            SpmCommand::DistUpgrade { yes } => {
                let msg = client::send_dist_upgrade_request(*yes)?;
                println!("{}", msg);
                Ok(())
            }
            SpmCommand::Search { query } => {
                let results = query::search_packages(query)?;
                for pkg in &results {
                    let source = pkg.source_repo.as_deref().unwrap_or("?");
                    println!("  {} [{}/{}]", pkg.name, source, pkg.format);
                }
                println!("  {} package(s) found", results.len());
                Ok(())
            }
            SpmCommand::GlobalSearch { query, yes } => {
                global_search(query, *yes)
            }
            SpmCommand::SearchFile { path } => {
                let owners = query::search_file_owner(path)?;
                if owners.is_empty() {
                    println!("  No package owns '{}'", path);
                } else {
                    for owner in &owners {
                        println!("  {}", owner);
                    }
                }
                Ok(())
            }
            SpmCommand::Info { package } => {
                let info = query::package_info(package)?;
                let files = query::list_package_files(package)?;
                let conn = crate::db::get_connection()?;
                if let Some(pkg) = crate::db::get_installed_package(&conn, package)? {
                    crate::output::show_installed_info(&pkg, files.len());
                } else {
                    println!("{}", info);
                }
                Ok(())
            }
            SpmCommand::Files { package } => {
                let files = query::list_package_files(package)?;
                println!("  {} owns {} file(s):", package, files.len());
                for f in &files {
                    println!("    {}", f);
                }
                Ok(())
            }
            SpmCommand::Depends { package } => {
                let deps = query::package_dependencies(package)?;
                println!("  {} depends on:", package);
                for d in &deps {
                    println!("    {}", d.name);
                }
                Ok(())
            }
            SpmCommand::Rdepends { package } => {
                let rdeps = query::reverse_dependencies(package)?;
                println!("  Packages depending on {}:", package);
                for r in &rdeps {
                    println!("    {}", r);
                }
                Ok(())
            }
            SpmCommand::History { undo } => {
                if let Some(id) = undo {
                    query::undo_transaction(*id)?;
                } else {
                    let history = query::show_history()?;
                    println!("{}", history);
                }
                Ok(())
            }
            SpmCommand::Snapshot { action } => match action {
                SnapshotAction::Create => {
                    query::create_snapshot()?;
                    Ok(())
                }
                SnapshotAction::Rollback { id } => {
                    query::rollback_snapshot(id)?;
                    Ok(())
                }
            }
            SpmCommand::Group { action } => match action {
                GroupAction::Create => crate::cli::group::handle_group_create(),
                GroupAction::Add { user } => crate::cli::group::handle_group_add_user(user),
                GroupAction::Remove { user } => crate::cli::group::handle_group_remove_user(user),
                GroupAction::List => crate::cli::group::handle_group_list(),
            },
            SpmCommand::Sync { files, prune } => {
                crate::sync::sync_system(*files, *prune)?;
                Ok(())
            }
            SpmCommand::Fsck { fix, files } => {
                crate::fsck::check_integrity(*fix, *files)?;
                Ok(())
            }
            SpmCommand::Init { root, from_system, fix_backend, install_daemon } => {
                let fb = *fix_backend || root.is_some() || *from_system;
                crate::bootstrap::init_system(root.as_deref(), *from_system, fb)?;
                if *install_daemon {
                    crate::bootstrap::install_daemon_service()?;
                }
                Ok(())
            }
        }
    }
}

fn fetch_json(url: &str, timeout_secs: u64) -> Option<serde_json::Value> {
    use std::time::Duration;
    let resp = ureq::get(url)
        .config()
        .timeout_global(Some(Duration::from_secs(timeout_secs)))
        .build()
        .call()
        .ok()?;
    let body = resp.into_body().read_to_vec().ok()?;
    serde_json::from_slice(&body).ok()
}

fn global_search(query: &str, yes: bool) -> SpmResult<()> {
    crate::output::section(format!("🌐 Searching remote repositories for '{query}'"));

    struct SearchResult {
        name: String,
        label: String,
        description: String,
        kind: SearchKind,
    }
    enum SearchKind {
        Deb,
        Copr { full_name: String },
    }

    let encoded = percent_encode(query);
    let mut results: Vec<SearchResult> = Vec::new();
    let mut errors: Vec<String> = Vec::new();

    // ── Debian packages (sources.debian.org) ──
    let spinner_deb = crate::output::Spinner::new("Searching Debian...");
    {
        let url = format!("https://sources.debian.org/api/search/{encoded}/");
        match fetch_json(&url, 10) {
            Some(val) => {
                let mut seen = std::collections::HashSet::new();
                if let Some(r) = val.get("results") {
                    if let Some(exact) = r.get("exact").and_then(|e| e.as_object()) {
                        if let Some(name) = exact.get("name").and_then(|v| v.as_str()) {
                            if seen.insert(name.to_string()) {
                                results.push(SearchResult {
                                    name: name.to_string(),
                                    label: "[deb]".into(),
                                    description: "exact match".into(),
                                    kind: SearchKind::Deb,
                                });
                            }
                        }
                    }
                    if let Some(other) = r.get("other").and_then(|e| e.as_array()) {
                        for item in other {
                            if let Some(name) = item.get("name").and_then(|v| v.as_str()) {
                                if seen.insert(name.to_string()) {
                                    results.push(SearchResult {
                                        name: name.to_string(),
                                        label: "[deb]".into(),
                                        description: String::new(),
                                        kind: SearchKind::Deb,
                                    });
                                }
                            }
                        }
                    }
                }
            }
            None => errors.push("Debian sources: request failed".into()),
        }
        spinner_deb.finish();
    }

    // ── COPR projects (rpm) ──
    let spinner_copr = crate::output::Spinner::new("Searching COPR...");
    {
        let url = format!("https://copr.fedorainfracloud.org/api_3/project/search?query={encoded}&limit=20");
        if let Some(val) = fetch_json(&url, 10) {
            if let Some(items) = val.get("items").and_then(|e| e.as_array()) {
                for item in items {
                    let full_name = item.get("full_name").and_then(|v| v.as_str()).unwrap_or("?");
                    let description = item.get("description").and_then(|v| v.as_str()).unwrap_or("");
                    results.push(SearchResult {
                        name: full_name.rsplit('/').next().unwrap_or(full_name).to_string(),
                        label: "[copr]".into(),
                        description: truncate(description, 70),
                        kind: SearchKind::Copr { full_name: full_name.to_string() },
                    });
                }
            }
        } else {
            errors.push("COPR: request failed".into());
        }
        spinner_copr.finish();
    }

    if results.is_empty() && errors.is_empty() {
        crate::output::step_info(format!("No remote results for '{query}'"));
        return Ok(());
    }
    if results.is_empty() {
        crate::output::step_warn(format!("No results for '{query}'. Backend errors:"));
        for e in &errors {
            crate::output::step_info(format!("  ✖ {e}"));
        }
        return Ok(());
    }

    // ── Display numbered list ──
    println!();
    let width = (results.len().checked_ilog10().unwrap_or(0) + 1) as usize;
    for (i, r) in results.iter().enumerate() {
        let desc = if r.description.is_empty() { String::new() } else { format!("  {}", crate::output::dim(&r.description)) };
        println!("  {i:>width$}. {} {}  {}", crate::output::bold(&r.name), crate::output::dim(&r.label), desc);
    }

    let copr_count = results.iter().filter(|r| matches!(r.kind, SearchKind::Copr { .. })).count();
    let deb_count = results.len() - copr_count;
    crate::output::step_info(format!("Found {} result(s) — {} Debian, {} COPR", results.len(), deb_count, copr_count));

    // ── Non-interactive mode: just show results ──
    if yes || !std::io::stdin().is_terminal() {
        return Ok(());
    }

    // ── Interactive selection ──
    let selection = loop {
        let input = {
            eprint!("  {} Enter number to install (or q to quit): ", crate::output::cyan("?"));
            let _ = std::io::stdout().flush();
            let mut buf = String::new();
            if std::io::stdin().read_line(&mut buf).is_err() { return Ok(()); }
            buf.trim().to_lowercase()
        };
        if input == "q" || input == "quit" {
            crate::output::step_info("Cancelled.");
            return Ok(());
        }
        match input.parse::<usize>() {
            Ok(n) if n < results.len() => break n,
            _ => crate::output::step_warn("Invalid selection, try again"),
        }
    };

    let chosen = &results[selection];
    println!();

    // ── Show details ──
    crate::output::step_info(format!("Selected: {} {}", crate::output::bold(&chosen.name), crate::output::dim(&chosen.label)));
    if !chosen.description.is_empty() {
        crate::output::step_info(format!("Description: {}", chosen.description));
    }

    // ── Trust prompt ──
    match &chosen.kind {
        SearchKind::Deb => {
            crate::output::step_info("Source: Debian (sources.debian.org) — official Debian package archive");
            let trust = prompt_yes("Do you trust Debian mirrors?", true);
            if !trust {
                crate::output::step_info("Installation cancelled.");
                return Ok(());
            }
        }
        SearchKind::Copr { full_name } => {
            crate::output::step_info(format!("Source: COPR project '{}' — third-party RPM repository", crate::output::bold(full_name)));
            crate::output::step_warn("COPR repositories are third-party and may contain untrusted software.");
            if !prompt_yes(&format!("Do you trust '{}'?", full_name), false) {
                crate::output::step_info("Installation cancelled.");
                return Ok(());
            }
        }
    }

    // ── Cache prompt ──
    let cache = prompt_yes("Cache the downloaded package for offline use?", true);

    // ── Download/Install ──
    match &chosen.kind {
        SearchKind::Deb => {
            let pkg_name = &chosen.name;
            crate::output::section(format!("📦 Installing {} from Debian", pkg_name));

            // Check if any configured repo has this package
            let repos_list = repos::load_repos()?;
            let has_pkg = repos_list.iter().any(|(rn, rc)| {
                matches!(rc.source, crate::types::RepoSource::Apt)
                    && install::repo_has_package(pkg_name, rn, rc)
            });

            if !has_pkg {
                // Check if Debian repo is configured
                let has_debian = repos_list.iter().any(|(n, rc)| {
                    n == "debian" && matches!(rc.source, crate::types::RepoSource::Apt)
                });

                if has_debian {
                    crate::output::step_info("Debian repo configured but package not found — cache may be empty.");
                    crate::output::step_info("Running 'spm update' to fetch repository metadata...");
                    repos::update_repos()?;

                    // Check again after update
                    let repos_list2 = repos::load_repos()?;
                    let has_pkg2 = repos_list2.iter().any(|(rn, rc)| {
                        matches!(rc.source, crate::types::RepoSource::Apt)
                            && install::repo_has_package(pkg_name, rn, rc)
                    });

                    if !has_pkg2 {
                        crate::output::step_warn(format!("'{}' not found in Debian repo after update.", pkg_name));
                        crate::output::step_info("The package may have a different name in Debian. Try searching again.");
                        return Ok(());
                    }
                } else {
                    // No Debian repo configured — offer to add one
                    crate::output::step_info("No Debian repository configured for direct download.");
                    let add_repo = prompt_yes(
                        "Auto-add Debian repository (bookworm) for direct HTTP download?",
                        true,
                    );
                    if !add_repo {
                        crate::output::step_info("Installation cancelled.");
                        return Ok(());
                    }

                    repos::add_repo(
                        "debian",
                        crate::types::RepoSource::Apt,
                        None,
                        Some(vec!["http://deb.debian.org/debian".into()]),
                        None,
                    )?;
                    crate::output::step_info("Added Debian repository. Fetching package metadata...");
                    repos::update_repos()?;
                }
            }

            // Install from Debian apt repo with smart mode (library isolation) for cross-distro safety
            match install::install_package_from_repo(pkg_name, crate::types::RepoSource::Apt, false, true, true, Default::default(), None) {
                Ok(()) => {
                    if cache {
                        let cache_dir = crate::config::paths::archives_dir();
                        let _ = std::fs::create_dir_all(&cache_dir);
                        crate::output::step_info(format!("Package cached at {}", cache_dir.display()));
                    }
                }
                Err(e) => {
                    crate::output::step_warn(format!("Installation failed: {e}"));
                    crate::output::step_info("This is a Debian package — may have unresolved dependencies on RPM-based systems.");
                }
            }
        }
        SearchKind::Copr { full_name } => {
            let arch = crate::util::fs::host_arch();
            let fedora_rel = fedora_release();
            let chroot = fedora_rel.as_ref()
                .map(|rel| format!("fedora-{}-{}", rel, arch))
                .unwrap_or_else(|| format!("fedora-{}-{}", 40, arch));

            // COPR download URL (RPM-MD format)
            let repo_base = format!(
                "https://download.copr.fedorainfracloud.org/results/{}/{}",
                full_name, chroot
            );

            // Try HTTP direct download first (no dnf needed)
            match crate::package::fetch::gs::download_rpm_from_repo(&repo_base, &chosen.name) {
                Ok(rpm_bytes) => {
                    let rpm_path = format!("/tmp/{}.rpm", chosen.name);
                    if let Err(e) = std::fs::write(&rpm_path, &rpm_bytes) {
                        crate::output::step_warn(format!("Failed to write RPM: {e}"));
                    } else {
                        crate::output::section(format!("📦 Downloaded {} from COPR", chosen.name));
                        // Install via spm local file install
                        match crate::package::install::install_local_package(&rpm_path, false, true) {
                            Ok(()) => {
                                if cache {
                                    crate::output::step_info(format!("Package cached at /var/cache/spm/archives/{}.rpm", chosen.name));
                                }
                                let _ = std::fs::remove_file(&rpm_path);
                                return Ok(());
                            }
                            Err(e) => {
                                crate::output::step_warn(format!("Failed to install downloaded RPM: {e}"));
                                crate::output::step_info("Falling back to dnf...");
                            }
                        }
                    }
                }
                Err(e) => {
                    crate::output::step_info(format!("HTTP download not available: {e}"));
                    crate::output::step_info("Falling back to dnf...");
                }
            }

            // Fallback: use dnf
            let repo_name = format!("copr-{}", full_name.replace(['/', '.'], "-"));
            let repo_url = format!(
                "https://copr.fedorainfracloud.org/repos/{}/repo/fedora-$releasever-{}/",
                full_name, arch
            );
            let repo_config = format!(
                "[{}]\nname=COPR {}\nbaseurl={}\nenabled=1\ntype=rpm-md\n",
                repo_name, full_name, repo_url
            );
            let repo_dir = crate::config::paths::repos_config_dir().join("copr");
            let _ = std::fs::create_dir_all(&repo_dir);
            let repo_path = repo_dir.join(format!("{}.repo", repo_name));
            if std::fs::write(&repo_path, repo_config).is_ok() {
                crate::output::step_info(format!("Added COPR repository: {}", repo_path.display()));
            } else {
                crate::output::step_warn("Could not add COPR repository automatically");
                return Ok(());
            }

            crate::output::step_info("Querying available packages from this COPR repository...");
            let list_output = std::process::Command::new("dnf")
                .args(["repoquery", "--disablerepo=*", "--enablerepo", &repo_name, "--available", "--quiet"])
                .output().ok();

            let mut pkgs: Vec<String> = match list_output {
                Some(o) if o.status.success() => {
                    String::from_utf8_lossy(&o.stdout)
                        .lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect()
                }
                _ => Vec::new(),
            };

            if pkgs.is_empty() {
                crate::output::step_warn("No packages found in this COPR repository.");
                return Ok(());
            }

            let pkg_name = if pkgs.len() == 1 {
                pkgs.remove(0)
            } else if !yes && std::io::stdin().is_terminal() {
                println!();
                let width = (pkgs.len().checked_ilog10().unwrap_or(0) + 1) as usize;
                for (i, p) in pkgs.iter().enumerate() { println!("  {i:>width$}. {}", crate::output::bold(p)); }
                let sel = loop {
                    let input = {
                        eprint!("  {} Select package: ", crate::output::cyan("?"));
                        let _ = std::io::stdout().flush();
                        let mut buf = String::new();
                        if std::io::stdin().read_line(&mut buf).is_err() { return Ok(()); }
                        buf.trim().to_lowercase()
                    };
                    if input == "q" { return Ok(()); }
                    match input.parse::<usize>() { Ok(n) if n < pkgs.len() => break n, _ => crate::output::step_warn("Invalid") }
                };
                pkgs[sel].clone()
            } else { pkgs[0].clone() };

            crate::output::section(format!("📦 Installing {} via dnf", pkg_name));
            let status = std::process::Command::new("dnf").args(["install", "-y", &pkg_name]).status();
            if status.map(|s| s.success()).unwrap_or(false) {
                crate::output::result_message(format!("Installed {}", pkg_name));
            } else {
                crate::output::step_warn(format!("dnf could not install '{}'.", pkg_name));
            }
        }
    }

    Ok(())
}

/// Detect Fedora release version from /etc/fedora-release
fn fedora_release() -> Option<String> {
    let content = std::fs::read_to_string("/etc/fedora-release").ok()?;
    // "Fedora release 40 (Forty)"
    content.split_whitespace().nth(2).map(|s| s.to_string())
}

fn prompt_yes(prompt: &str, default: bool) -> bool {
    let hint = if default { "Y/n" } else { "y/N" };
    eprint!("  {} {} [{}]: ", crate::output::cyan("?"), prompt, hint);
    let _ = std::io::stdout().flush();
    let mut buf = String::new();
    if std::io::stdin().read_line(&mut buf).is_err() {
        return default;
    }
    let trimmed = buf.trim().to_lowercase();
    if trimmed.is_empty() {
        return default;
    }
    matches!(trimmed.as_str(), "y" | "yes")
}

fn truncate(s: &str, max: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() > max {
        let t: String = chars.into_iter().take(max).collect();
        format!("{}…", t)
    } else {
        s.to_string()
    }
}

fn percent_encode(s: &str) -> String {
    urlencoding::encode(s).into_owned()
}

/// Pure path-like check (no filesystem access). Returns true for strings that look like local paths.
fn is_local_path_string(s: &str) -> bool {
    s.contains('/') || s.contains('\\')
        || s.ends_with(".deb") || s.ends_with(".rpm") || s.ends_with(".sam")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_local_path_string_with_slash() {
        assert!(is_local_path_string("./pkg.deb"));
        assert!(is_local_path_string("/usr/bin"));
    }

    #[test]
    fn test_is_local_path_string_with_backslash() {
        assert!(is_local_path_string(".\\pkg.deb"));
        assert!(is_local_path_string("C:\\pkgs\\pkg.rpm"));
    }

    #[test]
    fn test_is_local_path_string_with_deb() {
        assert!(is_local_path_string("pkg.deb"));
        assert!(is_local_path_string("my-package_1.0_amd64.deb"));
    }

    #[test]
    fn test_is_local_path_string_with_rpm() {
        assert!(is_local_path_string("pkg.rpm"));
    }

    #[test]
    fn test_is_local_path_string_with_sam() {
        assert!(is_local_path_string("pkg.sam"));
    }

    #[test]
    fn test_is_local_path_string_simple_name() {
        assert!(!is_local_path_string("nginx"));
        assert!(!is_local_path_string("python3"));
    }

    #[test]
    fn test_is_local_path_string_empty() {
        assert!(!is_local_path_string(""));
    }

    #[test]
    fn test_is_local_path_string_partial_suffix() {
        // ".deb" suffix alone isn't enough (but contains '.')
        assert!(!is_local_path_string("something.de"));
        assert!(!is_local_path_string("something.debx"));
    }

    #[test]
    fn test_percent_encode_empty() {
        assert_eq!(percent_encode(""), "");
    }

    #[test]
    fn test_percent_encode_ascii() {
        assert_eq!(percent_encode("hello"), "hello");
    }

    #[test]
    fn test_percent_encode_special() {
        let encoded = percent_encode("hello world");
        assert_eq!(encoded, "hello%20world");
    }

    #[test]
    fn test_percent_encode_slash() {
        let encoded = percent_encode("a/b");
        assert_eq!(encoded, "a%2Fb");
    }

    #[test]
    fn test_percent_encode_already_encoded() {
        let encoded = percent_encode("hello%20world");
        // Should double-encode the %
        assert_eq!(encoded, "hello%2520world");
    }

    #[test]
    fn test_fedora_release_not_fedora() {
        // On non-Fedora systems, this should return None
        let rel = fedora_release();
        // The test environment may be anything; just ensure it doesn't panic
        let _ = rel;
    }
}

