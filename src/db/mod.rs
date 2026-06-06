use std::fs;
use std::sync::{Mutex, OnceLock};

use fd_lock::RwLock;
use rusqlite::Connection;

pub mod conflict;
pub mod files;
pub mod mapping;
pub mod packages;
pub mod transactions;
pub mod users;

pub use files::*;
pub use mapping::*;
pub use packages::*;
pub use transactions::*;
pub use users::*;

use crate::config::paths;
use crate::error::{SpmError, SpmResult};

static DB: OnceLock<SpmResult<Mutex<Connection>>> = OnceLock::new();

pub fn get_connection() -> SpmResult<std::sync::MutexGuard<'static, Connection>> {
    let conn = DB.get_or_init(|| {
        open_db().map(Mutex::new)
    });
    match conn {
        Ok(c) => Ok(c.lock().map_err(|_| SpmError::other("DB mutex poisoned"))?),
        Err(e) => Err(SpmError::other(format!("DB init failed: {e}"))),
    }
}

pub fn with_write_lock<T>(f: impl FnOnce(&Connection) -> SpmResult<T>) -> SpmResult<T> {
    let lock_path = paths::metadata_db().with_extension("db.lock");
    let file = fs::File::create(&lock_path)?;
    let mut rwlock = RwLock::new(file);
    let _lock = rwlock.write()
        .map_err(|e| SpmError::other(format!("Cannot acquire write lock: {e}")))?;
    let conn = get_connection()?;
    f(&conn)
}

pub fn with_read_lock<T>(f: impl FnOnce(&Connection) -> SpmResult<T>) -> SpmResult<T> {
    let lock_path = paths::metadata_db().with_extension("db.lock");
    let file = fs::File::create(&lock_path)?;
    let rwlock = RwLock::new(file);
    let _lock = rwlock.read()
        .map_err(|e| SpmError::other(format!("Cannot acquire read lock: {e}")))?;
    let conn = get_connection()?;
    f(&conn)
}

pub fn open_db() -> SpmResult<Connection> {
    let db_path = paths::metadata_db();

    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let conn = Connection::open(&db_path)?;
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA foreign_keys=ON; PRAGMA busy_timeout=5000;")?;
    init_schema(&conn)?;
    Ok(conn)
}

pub fn init_schema(conn: &Connection) -> SpmResult<()> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS transactions (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            action TEXT NOT NULL,
            timestamp TEXT NOT NULL,
            user TEXT NOT NULL,
            status TEXT NOT NULL,
            packages TEXT NOT NULL,
            snapshot_id TEXT
        );

        CREATE TABLE IF NOT EXISTS files (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            transaction_id INTEGER NOT NULL,
            package TEXT NOT NULL,
            format TEXT NOT NULL DEFAULT 'Deb',
            filepath TEXT NOT NULL,
            hash TEXT NOT NULL,
            action TEXT NOT NULL,
            FOREIGN KEY (transaction_id) REFERENCES transactions(id)
        );

        CREATE TABLE IF NOT EXISTS installed_packages (
            name TEXT NOT NULL,
            version TEXT NOT NULL,
            format TEXT NOT NULL,
            install_type TEXT NOT NULL,
            manifest TEXT,
            install_date TEXT NOT NULL,
            source_repo TEXT,
            store_hash TEXT,
            PRIMARY KEY (name, format)
        );

        CREATE TABLE IF NOT EXISTS name_mappings (
            deb_name TEXT,
            rpm_name TEXT,
            lib_soname TEXT,
            UNIQUE(deb_name, rpm_name)
        );

        CREATE TABLE IF NOT EXISTS format_priority (
            format TEXT PRIMARY KEY,
            priority INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS user_installs (
            user_id INTEGER NOT NULL,
            package_name TEXT NOT NULL,
            package_format TEXT NOT NULL,
            package_hash TEXT NOT NULL,
            installed_at TEXT NOT NULL,
            PRIMARY KEY (user_id, package_name, package_format)
        );

        CREATE INDEX IF NOT EXISTS idx_files_transaction ON files(transaction_id);
        CREATE INDEX IF NOT EXISTS idx_files_package ON files(package, format);
        CREATE INDEX IF NOT EXISTS idx_transactions_timestamp ON transactions(timestamp);
        CREATE INDEX IF NOT EXISTS idx_user_installs_hash ON user_installs(package_hash);
        ",
    )?;

    conflict::init_conflict_schema(conn)?;

    let _ = conn.execute("ALTER TABLE installed_packages ADD COLUMN store_hash TEXT", []);
    let _ = conn.execute("ALTER TABLE installed_packages ADD COLUMN origin TEXT NOT NULL DEFAULT 'spm'", []);

    let _ = conn.execute_batch(
        "INSERT OR IGNORE INTO format_priority (format, priority) VALUES
            ('Deb', 10),
            ('Rpm', 20),
            ('Sam', 30);
        INSERT OR IGNORE INTO name_mappings (deb_name, rpm_name, lib_soname) VALUES
            ('libssl3', 'openssl-libs', 'libssl.so.3'),
            ('libssl1.1', 'openssl1.1-libs', 'libssl.so.1.1'),
            ('zlib1g', 'zlib', 'libz.so.1'),
            ('libpcre3', 'pcre', 'libpcre.so.3'),
            ('libcurl4', 'libcurl', 'libcurl.so.4'),
            ('libncurses6', 'ncurses', 'libncurses.so.6'),
            ('libreadline8', 'readline', 'libreadline.so.8'),
            ('libsqlite3-0', 'sqlite-libs', 'libsqlite3.so.0'),
            ('libxml2', 'libxml2', 'libxml2.so.2'),
            ('libexpat1', 'expat', 'libexpat.so.1'),
            ('libc6', 'glibc', 'libc.so.6'),
            ('libstdc++6', 'libstdc++', 'libstdc++.so.6'),
            ('zstd', 'libzstd', 'libzstd.so.1'),
            ('liblzma5', 'xz-libs', 'liblzma.so.5'),
            ('libbz2-1.0', 'bzip2-libs', 'libbz2.so.1.0'),
            ('libsystemd0', 'systemd-libs', 'libsystemd.so.0'),
            ('libcap2', 'libcap', 'libcap.so.2'),
            ('libpam0g', 'pam', 'libpam.so.0'),
            ('libgcc-s1', 'libgcc', 'libgcc_s.so.1'),
            ('libglib2.0-0', 'glib2', 'libglib-2.0.so.0'),
            ('libcrypt2', 'libxcrypt', 'libcrypt.so.2'),
            ('libelf1', 'elfutils-libelf', 'libelf.so.1'),
            ('libuuid1', 'libuuid', 'libuuid.so.1'),
            ('libselinux1', 'libselinux', 'libselinux.so.1'),
            ('libnss3', 'nss', 'libnss3.so'),
            ('libnspr4', 'nspr', 'libnspr4.so'),
            ('libcups2', 'cups-libs', 'libcups.so.2'),
            ('libdbus-1-3', 'dbus-libs', 'libdbus-1.so.3'),
            ('libpulse0', 'pulseaudio-libs', 'libpulse.so.0');
        ",
    );
    Ok(())
}

pub fn init_db() -> SpmResult<Connection> {
    let conn = open_db()?;
    init_schema(&conn)?;
    Ok(conn)
}

#[cfg(test)]
fn open_in_memory_db() -> SpmResult<Connection> {
    let conn = Connection::open_in_memory()?;
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
    init_schema(&conn)?;
    Ok(conn)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::*;

    #[test]
    fn test_open_in_memory() {
        let conn = open_in_memory_db().unwrap();
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM installed_packages", [], |r| r.get(0)).unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn test_add_and_get_installed_package() {
        let conn = open_in_memory_db().unwrap();
        let pkg = InstalledPackage {
            name: "nginx".into(),
            version: "1.27.0".into(),
            format: PackageFormat::Deb,
            install_type: InstallType::Native,
            manifest: None,
            install_date: "2026-05-30T12:00:00Z".into(),
            source_repo: Some("ubuntu".into()),
            store_hash: None,
            origin: InstallOrigin::Spm,
        };
        packages::add_installed_package(&conn, &pkg).unwrap();

        let retrieved = packages::get_installed_package(&conn, "nginx").unwrap().unwrap();
        assert_eq!(retrieved.name, "nginx");
        assert_eq!(retrieved.version, "1.27.0");
        assert!(matches!(retrieved.format, PackageFormat::Deb));
        assert!(matches!(retrieved.install_type, InstallType::Native));
    }

    #[test]
    fn test_get_nonexistent_package() {
        let conn = open_in_memory_db().unwrap();
        let result = packages::get_installed_package(&conn, "nonexistent").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_list_installed_packages() {
        let conn = open_in_memory_db().unwrap();
        let pkgs = packages::list_installed_packages(&conn).unwrap();
        assert!(pkgs.is_empty());

        packages::add_installed_package(&conn, &InstalledPackage {
            name: "a".into(), version: "1".into(), format: PackageFormat::Deb,
            install_type: InstallType::Native, manifest: None,
            install_date: "now".into(), source_repo: None,             store_hash: None,
            origin: InstallOrigin::Spm,
        }).unwrap();
        packages::add_installed_package(&conn, &InstalledPackage {
            name: "b".into(), version: "2".into(), format: PackageFormat::Rpm,
            install_type: InstallType::Native, manifest: None,
            install_date: "now".into(), source_repo: None,             store_hash: None,
            origin: InstallOrigin::Spm,
        }).unwrap();

        let pkgs = packages::list_installed_packages(&conn).unwrap();
        assert_eq!(pkgs.len(), 2);
        assert_eq!(pkgs[0].name, "a");
        assert_eq!(pkgs[1].name, "b");
    }

    #[test]
    fn test_remove_installed_package() {
        let conn = open_in_memory_db().unwrap();
        packages::add_installed_package(&conn, &InstalledPackage {
            name: "test".into(), version: "1".into(), format: PackageFormat::Sam,
            install_type: InstallType::Native, manifest: None,
            install_date: "now".into(), source_repo: None,             store_hash: None,
            origin: InstallOrigin::Spm,
        }).unwrap();
        packages::remove_installed_package(&conn, "test").unwrap();
        let result = packages::get_installed_package(&conn, "test").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_record_and_get_transaction() {
        let conn = open_in_memory_db().unwrap();
        let tx = Transaction {
            id: None,
            action: TransactionAction::Install,
            timestamp: "2026-05-30T12:00:00Z".into(),
            user: "root".into(),
            status: TransactionStatus::Completed,
            packages: vec!["nginx".into()],
            snapshot_id: None,
        };
        let tx_id = transactions::record_transaction(&conn, &tx).unwrap();
        assert!(tx_id > 0);

        let retrieved = transactions::get_transaction(&conn, tx_id).unwrap().unwrap();
        assert!(matches!(retrieved.action, TransactionAction::Install));
        assert_eq!(retrieved.user, "root");
        assert_eq!(retrieved.packages, vec!["nginx"]);
    }

    #[test]
    fn test_list_transactions() {
        let conn = open_in_memory_db().unwrap();
        let txs = transactions::list_transactions(&conn).unwrap();
        assert!(txs.is_empty());

        transactions::record_transaction(&conn, &Transaction {
            id: None, action: TransactionAction::Install,
            timestamp: "t1".into(), user: "root".into(),
            status: TransactionStatus::Completed,
            packages: vec!["a".into()], snapshot_id: None,
        }).unwrap();

        transactions::record_transaction(&conn, &Transaction {
            id: None, action: TransactionAction::Remove,
            timestamp: "t2".into(), user: "root".into(),
            status: TransactionStatus::Completed,
            packages: vec!["b".into()], snapshot_id: None,
        }).unwrap();

        let txs = transactions::list_transactions(&conn).unwrap();
        assert_eq!(txs.len(), 2);
    }

    #[test]
    fn test_update_transaction_status() {
        let conn = open_in_memory_db().unwrap();
        let tx_id = transactions::record_transaction(&conn, &Transaction {
            id: None, action: TransactionAction::Install,
            timestamp: "now".into(), user: "root".into(),
            status: TransactionStatus::Completed,
            packages: vec!["x".into()], snapshot_id: None,
        }).unwrap();

        transactions::update_transaction_status(&conn, tx_id, &TransactionStatus::Undone).unwrap();
        let tx = transactions::get_transaction(&conn, tx_id).unwrap().unwrap();
        assert!(matches!(tx.status, TransactionStatus::Undone));
    }

    #[test]
    fn test_record_and_get_files() {
        let conn = open_in_memory_db().unwrap();
        let tx_id = transactions::record_transaction(&conn, &Transaction {
            id: None, action: TransactionAction::Install,
            timestamp: "now".into(), user: "root".into(),
            status: TransactionStatus::Completed,
            packages: vec!["nginx".into()], snapshot_id: None,
        }).unwrap();

        let fls = vec![
            FileRecord {
                id: None, transaction_id: tx_id,
                package: "nginx".into(), format: PackageFormat::Deb,
                filepath: "/usr/sbin/nginx".into(),
                hash: "abc".into(), action: FileAction::Created,
            },
            FileRecord {
                id: None, transaction_id: tx_id,
                package: "nginx".into(), format: PackageFormat::Deb,
                filepath: "/etc/nginx/nginx.conf".into(),
                hash: "def".into(), action: FileAction::Created,
            },
        ];
        files::record_files(&conn, &fls).unwrap();

        let by_pkg = files::get_files_by_package(&conn, "nginx").unwrap();
        assert_eq!(by_pkg.len(), 2);
        assert_eq!(by_pkg[0].filepath, "/usr/sbin/nginx");

        let by_tx = files::get_files_for_transaction(&conn, tx_id).unwrap();
        assert_eq!(by_tx.len(), 2);
        assert_eq!(by_tx[1].filepath, "/etc/nginx/nginx.conf");
    }

    #[test]
    fn test_foreign_key_cascade() {
        let conn = open_in_memory_db().unwrap();
        let tx_id = transactions::record_transaction(&conn, &Transaction {
            id: None, action: TransactionAction::Install,
            timestamp: "now".into(), user: "root".into(),
            status: TransactionStatus::Completed,
            packages: vec!["x".into()], snapshot_id: None,
        }).unwrap();

        files::record_files(&conn, &[FileRecord {
            id: None, transaction_id: tx_id,
            package: "x".into(), format: PackageFormat::Deb,
            filepath: "/f".into(),
            hash: "h".into(), action: FileAction::Created,
        }]).unwrap();

        let files_result = files::get_files_for_transaction(&conn, tx_id).unwrap();
        assert_eq!(files_result.len(), 1);
        assert_eq!(files_result[0].package, "x");
    }

    #[test]
    fn test_coexisting_formats() {
        let conn = open_in_memory_db().unwrap();
        packages::add_installed_package(&conn, &InstalledPackage {
            name: "libssl".into(), version: "3.0_deb".into(), format: PackageFormat::Deb,
            install_type: InstallType::Native, manifest: None,
            install_date: "old".into(), source_repo: None,             store_hash: None,
            origin: InstallOrigin::Spm,
        }).unwrap();

        packages::add_installed_package(&conn, &InstalledPackage {
            name: "libssl".into(), version: "3.0_rpm".into(), format: PackageFormat::Rpm,
            install_type: InstallType::Native, manifest: None,
            install_date: "new".into(), source_repo: None,             store_hash: None,
            origin: InstallOrigin::Spm,
        }).unwrap();

        let all = packages::list_installed_packages(&conn).unwrap();
        assert_eq!(all.len(), 2);

        let deb = packages::get_installed_package_by_id(&conn, &PackageId::new("libssl", PackageFormat::Deb)).unwrap().unwrap();
        assert_eq!(deb.version, "3.0_deb");

        let rpm = packages::get_installed_package_by_id(&conn, &PackageId::new("libssl", PackageFormat::Rpm)).unwrap().unwrap();
        assert_eq!(rpm.version, "3.0_rpm");
    }

    #[test]
    fn test_installed_package_with_manifest() {
        let conn = open_in_memory_db().unwrap();
        let manifest = r#"{"name":"test"}"#;
        packages::add_installed_package(&conn, &InstalledPackage {
            name: "test".into(), version: "1".into(), format: PackageFormat::Sam,
            install_type: InstallType::Sandbox, manifest: Some(manifest.into()),
            install_date: "now".into(), source_repo: None,             store_hash: None,
            origin: InstallOrigin::Spm,
        }).unwrap();

        let pkg = packages::get_installed_package(&conn, "test").unwrap().unwrap();
        assert_eq!(pkg.manifest, Some(manifest.into()));
        assert!(matches!(pkg.install_type, InstallType::Sandbox));
    }

    #[test]
    fn test_name_mapping_resolution() {
        let conn = open_in_memory_db().unwrap();

        let mut stmt = conn.prepare(
            "INSERT INTO name_mappings (deb_name, rpm_name, lib_soname) VALUES (?1, ?2, ?3)",
        ).unwrap();
        stmt.execute(rusqlite::params!["custom-deb", "custom-rpm", "custom.so.1"]).unwrap();

        let mut stmt = conn.prepare(
            "SELECT rpm_name, lib_soname FROM name_mappings WHERE deb_name = ?1",
        ).unwrap();
        let mut rows = stmt.query(rusqlite::params!["custom-deb"]).unwrap();
        let row = rows.next().unwrap().unwrap();
        assert_eq!(row.get::<_, String>(0).unwrap(), "custom-rpm");
    }
}
