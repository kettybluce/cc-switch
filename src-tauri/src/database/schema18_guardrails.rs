//! SCHEMA 18 硬锁护栏。
//!
//! 本 fork 停在 18：没有 `migrate_v18_to_v19`，`proxy_config` CHECK 永不接纳
//! `app_type = 'pi'`（Pi 接管走 `settings.proxy_takeover_pi`），salvage 引入的
//! WAL / `busy_timeout=5000` / `synchronous=NORMAL` 必须仍在。迁移只允许
//! `user_version` 0..=17 → 18，禁止任何路径写出 19。

use super::tests::{LEGACY_SCHEMA_SQL, V3_8_SCHEMA_V1_SQL};
use super::*;
use rusqlite::{params, Connection, ErrorCode};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};
use tempfile::NamedTempFile;

const SCHEMA_RS: &str = include_str!("schema.rs");
const MOD_RS: &str = include_str!("mod.rs");
const MIGRATION_RS: &str = include_str!("migration.rs");
const PROXY_DAO_RS: &str = include_str!("dao/proxy.rs");

const EXPECTED_PROXY_CHECK: &str = "CHECK (app_type IN ('claude','codex','gemini','grokbuild'))";
const CANONICAL_PROXY_APPS: [&str; 4] = ["claude", "codex", "gemini", "grokbuild"];
const SALVAGE_BUSY_TIMEOUT_MS: i32 = 5_000;

// 编译期锁：有人把 SCHEMA_VERSION 改成 19（或别的值）时直接编不过。
const _: () = assert!(SCHEMA_VERSION == 18);
const _: () = assert!(SCHEMA_VERSION != 19);

fn is_check_constraint_error(err: &rusqlite::Error) -> bool {
    match err {
        rusqlite::Error::SqliteFailure(info, _) => {
            info.code == ErrorCode::ConstraintViolation
                || format!("{info:?}")
                    .to_ascii_lowercase()
                    .contains("constraint")
        }
        _ => err
            .to_string()
            .to_ascii_lowercase()
            .contains("check constraint"),
    }
}

fn proxy_config_ddl(conn: &Connection) -> String {
    conn.query_row(
        "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'proxy_config'",
        [],
        |row| row.get::<_, Option<String>>(0),
    )
    .expect("read proxy_config ddl")
    .unwrap_or_default()
}

fn proxy_app_types(conn: &Connection) -> Vec<String> {
    let mut stmt = conn
        .prepare("SELECT app_type FROM proxy_config ORDER BY app_type")
        .expect("prepare app_type list");
    let rows = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .expect("query app_types");
    rows.collect::<Result<Vec<_>, _>>()
        .expect("collect app_types")
}

fn insert_proxy_app(conn: &Connection, app_type: &str) -> rusqlite::Result<usize> {
    conn.execute(
        "INSERT INTO proxy_config (app_type) VALUES (?1)",
        params![app_type],
    )
}

fn assert_proxy_config_rejects(conn: &Connection, app_type: &str) {
    let err = insert_proxy_app(conn, app_type)
        .expect_err(&format!("proxy_config CHECK must reject {app_type:?}"));
    assert!(
        is_check_constraint_error(&err),
        "expected CHECK constraint failure for {app_type:?}, got {err}"
    );
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM proxy_config WHERE app_type = ?1",
            [app_type],
            |row| row.get(0),
        )
        .expect("count rejected app_type");
    assert_eq!(count, 0, "rejected app_type {app_type:?} must not persist");
}

fn assert_no_proxy_config_pi_row(conn: &Connection) {
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM proxy_config WHERE app_type = 'pi'",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0);
    assert_eq!(
        count, 0,
        "SCHEMA 18 must never store proxy_config.app_type='pi'"
    );
}

fn assert_schema18_lock(conn: &Connection) {
    assert_eq!(
        Database::get_user_version(conn).expect("user_version"),
        SCHEMA_VERSION
    );
    assert_eq!(SCHEMA_VERSION, 18);
    if Database::table_exists(conn, "proxy_config").expect("proxy_config exists?") {
        let ddl = proxy_config_ddl(conn);
        assert!(
            ddl.contains(EXPECTED_PROXY_CHECK),
            "proxy_config DDL must keep SCHEMA 18 CHECK, got: {ddl}"
        );
        assert!(
            !ddl.to_ascii_lowercase().contains("'pi'"),
            "proxy_config DDL must not mention pi, got: {ddl}"
        );
        assert_no_proxy_config_pi_row(conn);
        assert_proxy_config_rejects(conn, "pi");
        for app in CANONICAL_PROXY_APPS {
            conn.execute(
                "INSERT OR IGNORE INTO proxy_config (app_type) VALUES (?1)",
                params![app],
            )
            .unwrap_or_else(|e| panic!("canonical app {app} must remain insertable: {e}"));
        }
    }
    if Database::table_exists(conn, "session_log_sync").expect("session_log_sync exists?") {
        assert!(
            Database::has_column(conn, "session_log_sync", "last_byte_offset")
                .expect("byte cursor"),
            "v18 session_log_sync.last_byte_offset"
        );
        assert!(
            Database::has_column(conn, "session_log_sync", "last_tail_fingerprint")
                .expect("tail fingerprint"),
            "v18 session_log_sync.last_tail_fingerprint"
        );
    }
}

fn apply_startup_migrations(conn: &Connection) {
    Database::create_tables_on_conn(conn).expect("create_tables (startup order)");
    Database::apply_schema_migrations_on_conn(conn).expect("apply_schema_migrations");
}

fn pragma_i32(conn: &Connection, pragma: &str) -> i32 {
    conn.query_row(&format!("PRAGMA {pragma};"), [], |row| row.get(0))
        .unwrap_or_else(|e| panic!("read PRAGMA {pragma}: {e}"))
}

fn pragma_string(conn: &Connection, pragma: &str) -> String {
    conn.query_row(&format!("PRAGMA {pragma};"), [], |row| row.get(0))
        .unwrap_or_else(|e| panic!("read PRAGMA {pragma}: {e}"))
}

fn assert_salvage_file_pragmas(conn: &Connection) {
    let journal_mode = pragma_string(conn, "journal_mode");
    assert!(
        journal_mode.eq_ignore_ascii_case("wal"),
        "salvage WAL must stay on, got {journal_mode}"
    );
    assert_eq!(
        pragma_i32(conn, "busy_timeout"),
        SALVAGE_BUSY_TIMEOUT_MS,
        "salvage busy_timeout is 5000ms"
    );
    // SQLite: OFF=0, NORMAL=1, FULL=2, EXTRA=3
    assert_eq!(
        pragma_i32(conn, "synchronous"),
        1,
        "salvage synchronous=NORMAL"
    );
    assert_eq!(
        pragma_i32(conn, "foreign_keys"),
        1,
        "foreign_keys must stay ON"
    );
}

fn open_configured_file_db() -> (NamedTempFile, Connection) {
    let tmp = NamedTempFile::new().expect("temp db");
    let conn = Connection::open(tmp.path()).expect("open file db");
    Database::configure_connection(&conn, true).expect("salvage PRAGMAs");
    (tmp, conn)
}

fn migration_match_body() -> &'static str {
    let start = SCHEMA_RS
        .find("fn apply_schema_migrations_on_conn")
        .expect("apply_schema_migrations_on_conn must exist");
    let rest = &SCHEMA_RS[start..];
    let end = rest
        .find("fn migrate_v0_to_v1")
        .expect("migrate_v0_to_v1 must follow the dispatcher");
    &rest[..end]
}

fn parse_match_arms(body: &str) -> Vec<i32> {
    let mut arms = Vec::new();
    for line in body.lines() {
        let trimmed = line.trim();
        let Some(rest) = trimmed.strip_suffix("=> {") else {
            continue;
        };
        let rest = rest.trim();
        if rest == "_" {
            continue;
        }
        if let Ok(version) = rest.parse::<i32>() {
            arms.push(version);
        }
    }
    arms
}

fn assert_source_has_no_v19(source: &str, name: &str) {
    assert!(
        !source.contains("fn migrate_v18_to_v19"),
        "{name} must not define migrate_v18_to_v19"
    );
    assert!(
        !source.contains("migrate_v18_to_v19("),
        "{name} must not call migrate_v18_to_v19"
    );
    assert!(
        !source.contains("set_user_version(conn, 19)"),
        "{name} must not write user_version 19"
    );
    assert!(
        !source.contains("set_user_version(conn, SCHEMA_VERSION + 1)"),
        "{name} must not bump user_version past SCHEMA_VERSION"
    );
    assert!(
        !source.contains("SCHEMA_VERSION: i32 = 19"),
        "{name} must not set SCHEMA_VERSION to 19"
    );
    assert!(
        !source.contains("SCHEMA_VERSION: i32 = 18 + 1"),
        "{name} must not disguise SCHEMA_VERSION as 19"
    );
}

#[test]
fn schema_version_constant_is_eighteen_not_nineteen() {
    assert_eq!(SCHEMA_VERSION, 18);
    assert_ne!(SCHEMA_VERSION, 19);
    assert!(SCHEMA_VERSION < 19);
    assert!(MOD_RS.contains("pub const SCHEMA_VERSION: i32 = 18;"));
    assert!(MOD_RS.contains("Fork lock: stay on 18"));
}

#[test]
fn source_lock_forbids_migrate_v18_to_v19() {
    for (name, source) in [
        ("schema.rs", SCHEMA_RS),
        ("mod.rs", MOD_RS),
        ("migration.rs", MIGRATION_RS),
        ("dao/proxy.rs", PROXY_DAO_RS),
    ] {
        assert_source_has_no_v19(source, name);
    }
    assert!(
        SCHEMA_RS.contains("fn migrate_v17_to_v18"),
        "last hop must remain v17 → v18"
    );
    assert!(
        !SCHEMA_RS.contains("fn migrate_v18_"),
        "no migrate_v18_* helper may exist"
    );
}

#[test]
fn migration_dispatcher_covers_0_through_17_only() {
    let body = migration_match_body();
    let arms = parse_match_arms(body);
    assert_eq!(
        arms,
        (0..=17).collect::<Vec<_>>(),
        "dispatcher must list 0..=17 and nothing else, got {arms:?}"
    );
    assert!(
        body.contains("Self::migrate_v17_to_v18(conn)"),
        "last explicit hop must call migrate_v17_to_v18"
    );
    assert!(
        body.contains("Self::set_user_version(conn, 18)"),
        "last explicit hop must stop at user_version 18"
    );
    assert!(
        !body.contains("Self::set_user_version(conn, 19)"),
        "dispatcher must never write 19"
    );
    assert!(
        !body.contains("migrate_v18_to_v19"),
        "dispatcher must not mention v18→v19"
    );
    assert!(
        body.contains("version > SCHEMA_VERSION"),
        "future databases must be rejected before the loop"
    );
}

#[test]
fn apply_on_fresh_tables_lands_exactly_on_18() {
    let conn = Connection::open_in_memory().expect("memory");
    apply_startup_migrations(&conn);
    assert_schema18_lock(&conn);
    assert_eq!(
        proxy_app_types(&conn),
        ["claude", "codex", "gemini", "grokbuild"]
    );
}

#[test]
fn already_at_18_is_idempotent_and_does_not_grow() {
    let conn = Connection::open_in_memory().expect("memory");
    apply_startup_migrations(&conn);
    Database::apply_schema_migrations_on_conn(&conn).expect("re-apply");
    Database::apply_schema_migrations_on_conn(&conn).expect("re-apply again");
    assert_schema18_lock(&conn);
}

#[test]
fn rejects_future_user_versions_including_19() {
    for future in [19, 20, 21, 100, i32::MAX] {
        let conn = Connection::open_in_memory().expect("memory");
        Database::create_tables_on_conn(&conn).expect("create tables");
        Database::set_user_version(&conn, future).expect("set future version");
        let err = Database::apply_schema_migrations_on_conn(&conn)
            .expect_err(&format!("version {future} must be rejected"));
        let message = err.to_string();
        assert!(
            message.contains("数据库版本过新"),
            "future {future} should say 过新, got {message}"
        );
        assert!(
            message.contains(&future.to_string()),
            "error should include the stored version {future}: {message}"
        );
        assert!(
            message.contains("18"),
            "error should mention supported SCHEMA 18: {message}"
        );
        assert_eq!(
            Database::get_user_version(&conn).expect("version unchanged"),
            future,
            "rejection must not rewrite a too-new database"
        );
    }
}

#[test]
fn stored_user_version_exceeds_supported_detects_19() {
    let tmp = NamedTempFile::new().expect("temp db");
    {
        let conn = Connection::open(tmp.path()).expect("open");
        Database::set_user_version(&conn, 19).expect("set 19");
    }
    let detected =
        Database::stored_user_version_exceeds_supported(tmp.path()).expect("read too-new marker");
    assert_eq!(detected, Some(19));

    {
        let conn = Connection::open(tmp.path()).expect("reopen");
        Database::set_user_version(&conn, 18).expect("set 18");
    }
    let detected =
        Database::stored_user_version_exceeds_supported(tmp.path()).expect("read current marker");
    assert_eq!(detected, None);
}

#[test]
fn set_user_version_rejects_negative() {
    let conn = Connection::open_in_memory().expect("memory");
    let err = Database::set_user_version(&conn, -1).expect_err("negative version");
    assert!(
        err.to_string().contains("不能为负数"),
        "unexpected error: {err}"
    );
}

#[test]
fn raw_negative_user_version_is_unknown_not_a_path_to_19() {
    let conn = Connection::open_in_memory().expect("memory");
    Database::create_tables_on_conn(&conn).expect("create tables");
    conn.execute("PRAGMA user_version = -1;", [])
        .expect("sqlite allows raw negative user_version");
    let err = Database::apply_schema_migrations_on_conn(&conn)
        .expect_err("negative version is not in 0..=17");
    let message = err.to_string();
    assert!(
        message.contains("未知的数据库版本") || message.contains("数据库版本过新"),
        "negative version must fail closed, got {message}"
    );
    assert!(
        !message.contains("19"),
        "failure must not mention schema 19: {message}"
    );
}

#[test]
fn file_backed_salvage_enables_wal_busy_timeout_and_synchronous_normal() {
    let (_tmp, conn) = open_configured_file_db();
    assert_salvage_file_pragmas(&conn);
}

#[test]
fn memory_connection_still_sets_busy_timeout_when_skipping_wal() {
    let conn = Connection::open_in_memory().expect("memory");
    Database::configure_connection(&conn, false).expect("skip WAL");
    assert_eq!(pragma_i32(&conn, "busy_timeout"), SALVAGE_BUSY_TIMEOUT_MS);
    assert_eq!(pragma_i32(&conn, "foreign_keys"), 1);
    let journal_mode = pragma_string(&conn, "journal_mode");
    assert!(
        journal_mode.eq_ignore_ascii_case("memory"),
        "in-memory DBs cannot use WAL, got {journal_mode}"
    );
}

#[test]
fn vacuum_rebuild_restores_wal_and_busy_timeout() {
    let tmp = NamedTempFile::new().expect("temp db");
    let conn = Connection::open(tmp.path()).expect("open");
    conn.execute("PRAGMA auto_vacuum = NONE;", [])
        .expect("disable auto_vacuum");
    Database::create_tables_on_conn(&conn).expect("create tables");
    Database::configure_connection(&conn, true).expect("initial salvage PRAGMAs");
    assert_salvage_file_pragmas(&conn);

    // VACUUM rebuilds the file image and can drop WAL; salvage must put it back.
    let rebuilt =
        Database::ensure_incremental_auto_vacuum_on_conn(&conn).expect("rebuild auto_vacuum");
    assert!(rebuilt, "NONE → INCREMENTAL should VACUUM");
    assert_eq!(Database::get_auto_vacuum_mode(&conn).expect("mode"), 2);
    assert_salvage_file_pragmas(&conn);
}

#[test]
fn salvage_busy_timeout_waits_out_a_writer_instead_of_immediate_busy() {
    let tmp = NamedTempFile::new().expect("temp db");
    let path = tmp.path().to_path_buf();

    let holder = Connection::open(&path).expect("holder");
    Database::configure_connection(&holder, true).expect("holder salvage PRAGMAs");
    holder
        .execute_batch("CREATE TABLE t (id INTEGER PRIMARY KEY);")
        .expect("create t");
    holder.execute("BEGIN IMMEDIATE;", []).expect("holder lock");
    holder
        .execute("INSERT INTO t (id) VALUES (1);", [])
        .expect("holder insert");

    let rushed = Connection::open(&path).expect("rushed");
    // Force the historical SQLite default (0). rusqlite may apply its own
    // timeout; salvage is the 5000ms busy_timeout from configure_connection.
    rushed
        .busy_timeout(Duration::from_millis(0))
        .expect("clear busy_timeout");
    assert_eq!(pragma_i32(&rushed, "busy_timeout"), 0);
    let rushed_err = rushed
        .execute("BEGIN IMMEDIATE;", [])
        .expect_err("zero timeout must SQLITE_BUSY");
    assert!(
        matches!(
            rushed_err,
            rusqlite::Error::SqliteFailure(ref info, _) if info.code == ErrorCode::DatabaseBusy
        ) || rushed_err.to_string().to_ascii_lowercase().contains("busy"),
        "expected SQLITE_BUSY without salvage timeout, got {rushed_err}"
    );

    let started = Arc::new(AtomicBool::new(false));
    let started_flag = Arc::clone(&started);
    let waiter_path = path.clone();
    let handle = thread::spawn(move || {
        let waiter = Connection::open(&waiter_path).expect("waiter");
        Database::configure_connection(&waiter, true).expect("waiter salvage PRAGMAs");
        assert_eq!(pragma_i32(&waiter, "busy_timeout"), SALVAGE_BUSY_TIMEOUT_MS);
        started_flag.store(true, Ordering::SeqCst);
        let start = Instant::now();
        waiter.execute("BEGIN IMMEDIATE;", []).expect("waiter lock");
        waiter
            .execute("INSERT INTO t (id) VALUES (2);", [])
            .expect("waiter insert");
        waiter.execute("COMMIT;", []).expect("waiter commit");
        start.elapsed()
    });

    while !started.load(Ordering::SeqCst) {
        thread::sleep(Duration::from_millis(1));
    }
    thread::sleep(Duration::from_millis(150));
    holder.execute("COMMIT;", []).expect("holder commit");
    let waited = handle.join().expect("waiter thread");
    assert!(
        waited >= Duration::from_millis(100),
        "salvage busy_timeout should block until the writer commits, elapsed {waited:?}"
    );
}

#[test]
fn proxy_config_ddl_check_never_lists_pi() {
    let conn = Connection::open_in_memory().expect("memory");
    apply_startup_migrations(&conn);
    let ddl = proxy_config_ddl(&conn);
    assert!(ddl.contains(EXPECTED_PROXY_CHECK), "ddl={ddl}");
    assert!(!ddl.to_ascii_lowercase().contains("'pi'"), "ddl={ddl}");
    assert!(
        !PROXY_DAO_RS.contains("VALUES ('pi'"),
        "DAO must not seed a pi proxy_config row"
    );
}

#[test]
fn proxy_config_check_rejects_pi_and_lookalikes() {
    let conn = Connection::open_in_memory().expect("memory");
    apply_startup_migrations(&conn);

    let rejected = [
        "pi",
        "PI",
        "Pi",
        "pI",
        "pi ",
        " pi",
        "pi\n",
        "pi\t",
        "π",
        "ｐｉ",
        "opencode",
        "hermes",
        "claude-desktop",
        "claudedesktop",
        "minimax",
        "minimaxcode",
        "openai",
        "cursor",
        "grok",
        "gemini-cli",
        "CLAUDE",
        "Codex",
        "Gemini",
        "GrokBuild",
        "grok-build",
        "",
        "claude ",
        "codex ",
        "0",
        "1",
        "null",
        "NULL",
        "app_type",
        "claude;drop table proxy_config",
    ];
    for app_type in rejected {
        assert_proxy_config_rejects(&conn, app_type);
    }

    // SQLite TEXT PRIMARY KEY still allows NULL (unlike INTEGER PRIMARY KEY).
    // Either a constraint error or a NULL row is acceptable; it must never
    // materialize as app_type = 'pi'.
    match conn.execute("INSERT INTO proxy_config (app_type) VALUES (NULL)", []) {
        Ok(_) => {
            let null_rows: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM proxy_config WHERE app_type IS NULL",
                    [],
                    |row| row.get(0),
                )
                .expect("count NULL app_type");
            assert!(null_rows >= 1);
        }
        Err(err) => {
            assert!(
                err.to_string().to_ascii_lowercase().contains("constraint")
                    || err.to_string().to_ascii_lowercase().contains("not null"),
                "NULL must fail a constraint, got {err}"
            );
        }
    }
    assert_no_proxy_config_pi_row(&conn);
}

#[test]
fn proxy_config_check_accepts_only_the_four_canonical_apps() {
    let conn = Connection::open_in_memory().expect("memory");
    apply_startup_migrations(&conn);
    conn.execute("DELETE FROM proxy_config", [])
        .expect("clear seed rows");
    for app in CANONICAL_PROXY_APPS {
        insert_proxy_app(&conn, app).unwrap_or_else(|e| panic!("canonical {app} must insert: {e}"));
    }
    assert_eq!(
        proxy_app_types(&conn),
        ["claude", "codex", "gemini", "grokbuild"]
    );
    assert_proxy_config_rejects(&conn, "pi");
}

#[test]
fn memory_db_enforces_check_even_when_user_version_is_still_zero() {
    let db = Database::memory().expect("memory db");
    let conn = db.conn.lock().expect("lock");
    // Database::memory() 建的是 v18 形状的表，但不跑 apply_schema_migrations，
    // user_version 可能仍是 0。CHECK 必须照样拒绝 pi。
    let version = Database::get_user_version(&conn).expect("version");
    assert!(
        version == 0 || version == SCHEMA_VERSION,
        "memory() should not invent a future schema, got {version}"
    );
    assert_no_proxy_config_pi_row(&conn);
    assert_proxy_config_rejects(&conn, "pi");
    let ddl = proxy_config_ddl(&conn);
    assert!(ddl.contains(EXPECTED_PROXY_CHECK), "ddl={ddl}");
}

#[tokio::test]
async fn pi_takeover_persists_in_settings_and_never_inserts_proxy_config_row() {
    let db = Database::memory().expect("memory db");
    assert!(!db.has_proxy_config_row("pi").expect("pi row?"));
    db.set_app_takeover_enabled("pi", true)
        .await
        .expect("enable pi takeover");
    assert!(db.is_pi_takeover_enabled().expect("flag"));
    assert!(db
        .is_app_takeover_enabled("pi")
        .await
        .expect("takeover reads settings"));
    assert!(
        !db.has_proxy_config_row("pi").expect("pi row after enable"),
        "Pi takeover must stay in settings.proxy_takeover_pi"
    );
    db.set_app_takeover_enabled("pi", false)
        .await
        .expect("disable pi takeover");
    assert!(!db.is_pi_takeover_enabled().expect("flag off"));
    assert!(!db.has_proxy_config_row("pi").expect("pi row after disable"));
    assert_eq!(SCHEMA_VERSION, 18);
}

#[test]
fn providers_table_may_store_pi_cards_without_a_proxy_config_row() {
    let conn = Connection::open_in_memory().expect("memory");
    apply_startup_migrations(&conn);
    conn.execute(
        "INSERT INTO providers (id, app_type, name, settings_config, meta)
         VALUES ('pi-card', 'pi', 'Pi Native', '{}', '{}')",
        [],
    )
    .expect("providers.app_type has no CHECK; Pi cards are allowed");
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM providers WHERE app_type = 'pi'",
            [],
            |row| row.get(0),
        )
        .expect("count pi providers");
    assert_eq!(count, 1);
    assert_no_proxy_config_pi_row(&conn);
    assert_proxy_config_rejects(&conn, "pi");
}

#[test]
fn ladder_from_every_version_0_through_18_on_current_ddl() {
    for start in 0..=SCHEMA_VERSION {
        let conn = Connection::open_in_memory().expect("memory");
        Database::create_tables_on_conn(&conn).expect("current DDL");
        conn.execute(
            "INSERT OR REPLACE INTO providers (id, app_type, name, settings_config, meta)
             VALUES ('pi-card', 'pi', 'Pi Native', '{}', '{}')",
            [],
        )
        .expect("sentinel pi provider");
        conn.execute(
            "INSERT OR REPLACE INTO settings (key, value) VALUES ('proxy_takeover_pi', 'true')",
            [],
        )
        .expect("sentinel takeover flag");
        Database::set_user_version(&conn, start).expect("set start version");
        Database::apply_schema_migrations_on_conn(&conn).unwrap_or_else(|e| {
            panic!("current-DDL ladder from v{start} must reach 18: {e}");
        });
        assert_schema18_lock(&conn);
        let pi_providers: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM providers WHERE id = 'pi-card' AND app_type = 'pi'",
                [],
                |row| row.get(0),
            )
            .expect("sentinel survived");
        assert_eq!(pi_providers, 1, "v{start} → 18 must keep Pi provider cards");
        let flag: String = conn
            .query_row(
                "SELECT value FROM settings WHERE key = 'proxy_takeover_pi'",
                [],
                |row| row.get(0),
            )
            .expect("takeover flag");
        assert_eq!(flag, "true", "v{start} → 18 must keep proxy_takeover_pi");
    }
}

#[test]
fn historical_v0_legacy_and_v1_v38_snapshots_land_on_18() {
    for (label, sql, version) in [
        ("v0-legacy", LEGACY_SCHEMA_SQL, 0),
        ("v1-v3.8", V3_8_SCHEMA_V1_SQL, 1),
    ] {
        let conn = Connection::open_in_memory().expect("memory");
        conn.execute("PRAGMA foreign_keys = ON;", [])
            .expect("foreign_keys");
        conn.execute_batch(sql)
            .unwrap_or_else(|e| panic!("seed {label}: {e}"));
        Database::set_user_version(&conn, version).expect("set version");
        apply_startup_migrations(&conn);
        assert_schema18_lock(&conn);
        assert!(
            Database::table_exists(&conn, "session_usage_dedup").expect("dedup table"),
            "{label} must gain the v17 ledger"
        );
        assert!(
            Database::table_exists(&conn, "profiles").expect("profiles table"),
            "{label} must gain the v12 profiles table"
        );
    }
}

#[test]
fn sparse_snapshots_from_v4_v10_v16_v17_only_upgrade_to_18() {
    let cases: &[(&str, i32, &str)] = &[
        (
            "v4-pricing-shape",
            4,
            r#"
            CREATE TABLE providers (
                id TEXT NOT NULL,
                app_type TEXT NOT NULL,
                name TEXT NOT NULL,
                settings_config TEXT NOT NULL DEFAULT '{}',
                meta TEXT NOT NULL DEFAULT '{}',
                PRIMARY KEY (id, app_type)
            );
            CREATE TABLE mcp_servers (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                server_config TEXT NOT NULL,
                enabled_claude INTEGER NOT NULL DEFAULT 0,
                enabled_codex INTEGER NOT NULL DEFAULT 0,
                enabled_gemini INTEGER NOT NULL DEFAULT 0,
                enabled_opencode INTEGER NOT NULL DEFAULT 0
            );
            CREATE TABLE skills (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL DEFAULT '',
                directory TEXT NOT NULL DEFAULT '',
                enabled_claude BOOLEAN NOT NULL DEFAULT 0
            );
            CREATE TABLE proxy_config (
                app_type TEXT PRIMARY KEY
                    CHECK (app_type IN ('claude','codex','gemini')),
                max_retries INTEGER NOT NULL DEFAULT 3,
                streaming_first_byte_timeout INTEGER NOT NULL DEFAULT 60,
                streaming_idle_timeout INTEGER NOT NULL DEFAULT 120,
                non_streaming_timeout INTEGER NOT NULL DEFAULT 600,
                circuit_failure_threshold INTEGER NOT NULL DEFAULT 4,
                circuit_success_threshold INTEGER NOT NULL DEFAULT 2,
                circuit_timeout_seconds INTEGER NOT NULL DEFAULT 60,
                circuit_error_rate_threshold REAL NOT NULL DEFAULT 0.6,
                circuit_min_requests INTEGER NOT NULL DEFAULT 10
            );
            INSERT INTO proxy_config (app_type) VALUES ('claude'), ('codex'), ('gemini');
            CREATE TABLE proxy_request_logs (
                request_id TEXT PRIMARY KEY,
                provider_id TEXT NOT NULL DEFAULT '',
                app_type TEXT NOT NULL DEFAULT 'claude',
                model TEXT NOT NULL,
                created_at INTEGER NOT NULL DEFAULT 0,
                session_id TEXT,
                status_code INTEGER NOT NULL DEFAULT 200
            );
            "#,
        ),
        (
            "v10-rollup-shape",
            10,
            r#"
            CREATE TABLE providers (
                id TEXT NOT NULL, app_type TEXT NOT NULL, name TEXT NOT NULL,
                settings_config TEXT NOT NULL DEFAULT '{}', meta TEXT NOT NULL DEFAULT '{}',
                PRIMARY KEY (id, app_type)
            );
            CREATE TABLE mcp_servers (
                id TEXT PRIMARY KEY, name TEXT NOT NULL, server_config TEXT NOT NULL
            );
            CREATE TABLE skills (
                id TEXT PRIMARY KEY, name TEXT NOT NULL DEFAULT '', directory TEXT NOT NULL DEFAULT '',
                enabled_claude BOOLEAN NOT NULL DEFAULT 0
            );
            CREATE TABLE proxy_config (
                app_type TEXT PRIMARY KEY
                    CHECK (app_type IN ('claude','codex','gemini','grokbuild')),
                max_retries INTEGER NOT NULL DEFAULT 3,
                streaming_first_byte_timeout INTEGER NOT NULL DEFAULT 60,
                streaming_idle_timeout INTEGER NOT NULL DEFAULT 120,
                non_streaming_timeout INTEGER NOT NULL DEFAULT 600,
                circuit_failure_threshold INTEGER NOT NULL DEFAULT 4,
                circuit_success_threshold INTEGER NOT NULL DEFAULT 2,
                circuit_timeout_seconds INTEGER NOT NULL DEFAULT 60,
                circuit_error_rate_threshold REAL NOT NULL DEFAULT 0.6,
                circuit_min_requests INTEGER NOT NULL DEFAULT 10
            );
            INSERT INTO proxy_config (app_type) VALUES ('claude'), ('codex'), ('gemini');
            CREATE TABLE proxy_request_logs (
                request_id TEXT PRIMARY KEY,
                provider_id TEXT NOT NULL DEFAULT '',
                app_type TEXT NOT NULL DEFAULT 'claude',
                model TEXT NOT NULL,
                request_model TEXT,
                created_at INTEGER NOT NULL DEFAULT 0,
                session_id TEXT,
                status_code INTEGER NOT NULL DEFAULT 200
            );
            CREATE TABLE usage_daily_rollups (
                date TEXT NOT NULL, app_type TEXT NOT NULL, provider_id TEXT NOT NULL,
                model TEXT NOT NULL, request_count INTEGER NOT NULL DEFAULT 0,
                success_count INTEGER NOT NULL DEFAULT 0, input_tokens INTEGER NOT NULL DEFAULT 0,
                output_tokens INTEGER NOT NULL DEFAULT 0, cache_read_tokens INTEGER NOT NULL DEFAULT 0,
                cache_creation_tokens INTEGER NOT NULL DEFAULT 0,
                total_cost_usd TEXT NOT NULL DEFAULT '0', avg_latency_ms INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY (date, app_type, provider_id, model)
            );
            INSERT INTO usage_daily_rollups
                (date, app_type, provider_id, model, request_count)
            VALUES ('2026-05-01', 'claude', 'p1', 'kimi-k2', 3);
            "#,
        ),
        (
            "v16-pre-dedup",
            16,
            r#"
            CREATE TABLE session_log_sync (
                file_path TEXT PRIMARY KEY,
                last_modified INTEGER NOT NULL,
                last_line_offset INTEGER NOT NULL DEFAULT 0,
                last_synced_at INTEGER NOT NULL
            );
            INSERT INTO session_log_sync VALUES ('/tmp/a.jsonl', 1, 2, 3);
            "#,
        ),
        (
            "v17-pre-byte-cursor",
            17,
            r#"
            CREATE TABLE session_log_sync (
                file_path TEXT PRIMARY KEY,
                last_modified INTEGER NOT NULL,
                last_line_offset INTEGER NOT NULL DEFAULT 0,
                last_synced_at INTEGER NOT NULL
            );
            INSERT INTO session_log_sync VALUES ('/tmp/a.jsonl', 5, 3, 1);
            "#,
        ),
    ];

    for (label, version, sql) in cases {
        let conn = Connection::open_in_memory().expect("memory");
        conn.execute_batch(sql)
            .unwrap_or_else(|e| panic!("seed {label}: {e}"));
        Database::set_user_version(&conn, *version).expect("set version");
        apply_startup_migrations(&conn);
        assert_schema18_lock(&conn);
        assert_eq!(
            Database::get_user_version(&conn).expect("version"),
            18,
            "{label} must stop at 18"
        );
    }
}

#[test]
fn v13_three_app_check_rebuilds_to_include_grokbuild_and_still_rejects_pi() {
    let conn = Connection::open_in_memory().expect("memory");
    conn.execute_batch(
        r#"
        CREATE TABLE proxy_config (
            app_type TEXT PRIMARY KEY CHECK (app_type IN ('claude','codex','gemini')),
            proxy_enabled INTEGER NOT NULL DEFAULT 0,
            listen_address TEXT NOT NULL DEFAULT '127.0.0.1',
            listen_port INTEGER NOT NULL DEFAULT 15721,
            enable_logging INTEGER NOT NULL DEFAULT 1,
            enabled INTEGER NOT NULL DEFAULT 0,
            auto_failover_enabled INTEGER NOT NULL DEFAULT 0,
            max_retries INTEGER NOT NULL DEFAULT 3,
            streaming_first_byte_timeout INTEGER NOT NULL DEFAULT 60,
            streaming_idle_timeout INTEGER NOT NULL DEFAULT 120,
            non_streaming_timeout INTEGER NOT NULL DEFAULT 600,
            circuit_failure_threshold INTEGER NOT NULL DEFAULT 4,
            circuit_success_threshold INTEGER NOT NULL DEFAULT 2,
            circuit_timeout_seconds INTEGER NOT NULL DEFAULT 60,
            circuit_error_rate_threshold REAL NOT NULL DEFAULT 0.6,
            circuit_min_requests INTEGER NOT NULL DEFAULT 10,
            default_cost_multiplier TEXT NOT NULL DEFAULT '1',
            pricing_model_source TEXT NOT NULL DEFAULT 'response',
            live_takeover_active INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        INSERT INTO proxy_config (app_type, enabled, max_retries)
        VALUES ('claude', 1, 9), ('codex', 0, 3), ('gemini', 0, 5);
        "#,
    )
    .expect("seed v13 3-app CHECK");
    Database::set_user_version(&conn, 13).expect("v13");
    apply_startup_migrations(&conn);
    assert_schema18_lock(&conn);
    let grok: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM proxy_config WHERE app_type = 'grokbuild'",
            [],
            |row| row.get(0),
        )
        .expect("grokbuild row");
    assert_eq!(grok, 1, "v14 must add grokbuild");
    let claude_retries: i64 = conn
        .query_row(
            "SELECT max_retries FROM proxy_config WHERE app_type = 'claude'",
            [],
            |row| row.get(0),
        )
        .expect("preserved claude retries");
    assert_eq!(claude_retries, 9);
    assert_proxy_config_rejects(&conn, "pi");
}

#[test]
fn v13_pi_row_without_check_fails_closed_and_never_reaches_18() {
    let conn = Connection::open_in_memory().expect("memory");
    conn.execute_batch(
        r#"
        CREATE TABLE proxy_config (
            app_type TEXT PRIMARY KEY,
            proxy_enabled INTEGER NOT NULL DEFAULT 0,
            listen_address TEXT NOT NULL DEFAULT '127.0.0.1',
            listen_port INTEGER NOT NULL DEFAULT 15721,
            enable_logging INTEGER NOT NULL DEFAULT 1,
            enabled INTEGER NOT NULL DEFAULT 0,
            auto_failover_enabled INTEGER NOT NULL DEFAULT 0,
            max_retries INTEGER NOT NULL DEFAULT 3,
            streaming_first_byte_timeout INTEGER NOT NULL DEFAULT 60,
            streaming_idle_timeout INTEGER NOT NULL DEFAULT 120,
            non_streaming_timeout INTEGER NOT NULL DEFAULT 600,
            circuit_failure_threshold INTEGER NOT NULL DEFAULT 4,
            circuit_success_threshold INTEGER NOT NULL DEFAULT 2,
            circuit_timeout_seconds INTEGER NOT NULL DEFAULT 60,
            circuit_error_rate_threshold REAL NOT NULL DEFAULT 0.6,
            circuit_min_requests INTEGER NOT NULL DEFAULT 10,
            default_cost_multiplier TEXT NOT NULL DEFAULT '1',
            pricing_model_source TEXT NOT NULL DEFAULT 'response',
            live_takeover_active INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            updated_at TEXT NOT NULL DEFAULT (datetime('now'))
        );
        INSERT INTO proxy_config (app_type) VALUES ('claude'), ('pi');
        "#,
    )
    .expect("seed illegal pre-check pi row");
    Database::set_user_version(&conn, 13).expect("v13");

    // create_tables 是 IF NOT EXISTS，不会丢掉非法行；v14 重建带 CHECK 的表时
    // 复制 pi 行必须失败，savepoint 回滚，user_version 停在 13。
    Database::create_tables_on_conn(&conn).expect("create_tables leaves old table");
    let err = Database::apply_schema_migrations_on_conn(&conn)
        .expect_err("copying a pi row into the v14 CHECK table must fail");
    let message = err.to_string().to_ascii_lowercase();
    assert!(
        message.contains("check") || message.contains("constraint"),
        "expected CHECK failure while copying pi, got {err}"
    );
    assert_eq!(
        Database::get_user_version(&conn).expect("rolled back version"),
        13,
        "failed v13→v14 must not leave the database at 18/19"
    );
}

#[test]
fn v17_without_session_log_sync_table_still_reaches_18() {
    let conn = Connection::open_in_memory().expect("memory");
    Database::set_user_version(&conn, 17).expect("v17");
    apply_startup_migrations(&conn);
    assert_schema18_lock(&conn);
}

#[test]
fn v17_that_already_has_byte_cursor_columns_stays_at_18() {
    let conn = Connection::open_in_memory().expect("memory");
    Database::create_tables_on_conn(&conn).expect("current DDL already has v18 columns");
    Database::set_user_version(&conn, 17).expect("v17");
    Database::apply_schema_migrations_on_conn(&conn).expect("v17→v18 no-op columns");
    assert_schema18_lock(&conn);
}

#[test]
fn fuzz_noise_tables_unicode_settings_and_missing_optional_tables() {
    for start in [0, 2, 5, 8, 11, 13, 15, 17] {
        let conn = Connection::open_in_memory().expect("memory");
        Database::create_tables_on_conn(&conn).expect("current DDL");
        conn.execute_batch(
            r#"
            CREATE TABLE IF NOT EXISTS zzz_noise (id INTEGER PRIMARY KEY, blob TEXT);
            INSERT INTO zzz_noise (blob) VALUES (char(0x1F600));
            INSERT OR REPLACE INTO settings (key, value)
            VALUES ('noise-键-🔑', json_array('pi', 'v19'));
            CREATE TABLE IF NOT EXISTS "Pragma; DROP TABLE providers" (x TEXT);
            "#,
        )
        .expect("seed noise");
        Database::set_user_version(&conn, start).expect("set start");
        Database::apply_schema_migrations_on_conn(&conn)
            .unwrap_or_else(|e| panic!("noise ladder from v{start}: {e}"));
        assert_schema18_lock(&conn);
        let noise: i64 = conn
            .query_row("SELECT COUNT(*) FROM zzz_noise", [], |row| row.get(0))
            .expect("noise table survived");
        assert_eq!(noise, 1, "migrations must not sweep unrelated tables");
    }
}

#[test]
fn file_backed_wal_migration_from_v16_keeps_salvage_pragmas() {
    let (_tmp, conn) = open_configured_file_db();
    Database::create_tables_on_conn(&conn).expect("tables");
    Database::set_user_version(&conn, 16).expect("v16");
    conn.execute(
        "INSERT INTO session_log_sync (file_path, last_modified, last_line_offset, last_synced_at)
         VALUES ('/tmp/wal.jsonl', 1, 1, 1)",
        [],
    )
    .ok();
    Database::apply_schema_migrations_on_conn(&conn).expect("v16→18 on WAL file");
    assert_schema18_lock(&conn);
    assert_salvage_file_pragmas(&conn);
}

#[test]
fn failed_migration_savepoint_does_not_bump_past_18() {
    let conn = Connection::open_in_memory().expect("memory");
    Database::create_tables_on_conn(&conn).expect("tables");
    Database::set_user_version(&conn, 19).expect("too new");
    let _ = Database::apply_schema_migrations_on_conn(&conn);
    assert_eq!(
        Database::get_user_version(&conn).expect("version"),
        19,
        "too-new DBs must be left untouched"
    );
}

#[test]
fn empty_database_at_v0_without_legacy_tables_fails_closed() {
    let conn = Connection::open_in_memory().expect("memory");
    Database::set_user_version(&conn, 0).expect("v0");
    // 不先 create_tables：v0→v1 需要旧表。失败必须停在 0，不能跳到 19。
    let err = Database::apply_schema_migrations_on_conn(&conn)
        .expect_err("empty v0 without tables cannot migrate");
    assert!(
        !err.to_string().contains("19"),
        "empty v0 failure must not mention 19: {err}"
    );
    assert_eq!(
        Database::get_user_version(&conn).expect("version"),
        0,
        "savepoint rollback must restore v0"
    );
}
