use sqlx::{migrate::MigrateDatabase, Result, Row, Sqlite, SqlitePool, Transaction};
use std::fs;
use std::path::Path;
use tauri::Manager;

#[derive(Clone)]
pub struct DatabaseManager {
    pool: SqlitePool,
}

impl DatabaseManager {
    pub async fn new(tauri_db_path: &str, backend_db_path: &str) -> Result<Self> {
        if let Some(parent_dir) = Path::new(tauri_db_path).parent() {
            if !parent_dir.exists() {
                fs::create_dir_all(parent_dir).map_err(|e| sqlx::Error::Io(e))?;
            }
        }

        if !Path::new(tauri_db_path).exists() {
            if Path::new(backend_db_path).exists() {
                log::info!(
                    "Copying database from {} to {}",
                    backend_db_path,
                    tauri_db_path
                );
                fs::copy(backend_db_path, tauri_db_path).map_err(|e| sqlx::Error::Io(e))?;
            } else {
                log::info!("Creating database at {}", tauri_db_path);
                Sqlite::create_database(tauri_db_path).await?;
            }
        }

        let pool = SqlitePool::connect(tauri_db_path).await?;

        let migrator = sqlx::migrate!("./migrations");
        reconcile_migration_checksums(&pool, &migrator).await?;
        run_migrations_with_recovery(&pool, &migrator).await?;

        Ok(DatabaseManager { pool })
    }

    // NOTE: So for the first time users they needs to start the application
    // after they can just delete the existing .sqlite file and then copy the existing .db file to
    // the current app dir, So the system detects legacy db and copy it and starts with that data
    // (Newly created .sqlite with the copied content from .db)
    pub async fn new_from_app_handle(app_handle: &tauri::AppHandle) -> Result<Self> {
        // Resolve the app's data directory
        let app_data_dir = app_handle
            .path()
            .app_data_dir()
            .map_err(|e| {
                sqlx::Error::Io(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    format!("failed to get app data dir: {}", e),
                ))
            })?;
        if !app_data_dir.exists() {
            fs::create_dir_all(&app_data_dir).map_err(|e| sqlx::Error::Io(e))?;
        }

        // Define database paths
        let tauri_db_path = app_data_dir
            .join("meeting_minutes.sqlite")
            .to_string_lossy()
            .to_string();
        // Legacy backend DB path (for auto-migration if exists)
        let backend_db_path = app_data_dir
            .join("meeting_minutes.db")
            .to_string_lossy()
            .to_string();

        // WAL file paths for defensive cleanup
        let wal_path = app_data_dir.join("meeting_minutes.sqlite-wal");
        let shm_path = app_data_dir.join("meeting_minutes.sqlite-shm");

        log::info!("Tauri DB path: {}", tauri_db_path);
        log::info!("Legacy backend DB path: {}", backend_db_path);

        // Try to open database with defensive WAL handling
        match Self::new(&tauri_db_path, &backend_db_path).await {
            Ok(db_manager) => {
                log::info!("Database opened successfully");
                Ok(db_manager)
            }
            Err(e) => {
                // Check if error is due to corrupted WAL file
                let error_msg = e.to_string();
                if error_msg.contains("malformed") || error_msg.contains("corrupt") {
                    log::warn!("Database appears corrupted, likely due to orphaned WAL file. Attempting recovery...");
                    log::warn!("Error details: {}", error_msg);

                    // Delete potentially corrupted WAL/SHM files
                    if wal_path.exists() {
                        match fs::remove_file(&wal_path) {
                            Ok(_) => log::info!("Removed orphaned WAL file: {:?}", wal_path),
                            Err(e) => log::warn!("Failed to remove WAL file: {}", e),
                        }
                    }
                    if shm_path.exists() {
                        match fs::remove_file(&shm_path) {
                            Ok(_) => log::info!("Removed orphaned SHM file: {:?}", shm_path),
                            Err(e) => log::warn!("Failed to remove SHM file: {}", e),
                        }
                    }

                    // Retry connection without WAL files
                    log::info!("Retrying database connection after WAL cleanup...");
                    match Self::new(&tauri_db_path, &backend_db_path).await {
                        Ok(db_manager) => {
                            log::info!("Database opened successfully after WAL recovery");
                            Ok(db_manager)
                        }
                        Err(retry_err) => {
                            log::error!("Database connection failed even after WAL cleanup: {}", retry_err);
                            Err(retry_err)
                        }
                    }
                } else {
                    // Not a WAL-related error, propagate original error
                    log::error!("Database connection failed: {}", error_msg);
                    Err(e)
                }
            }
        }
    }

    /// Check if this is the first launch (sqlite database doesn't exist yet)
    pub async fn is_first_launch(app_handle: &tauri::AppHandle) -> Result<bool> {
        let app_data_dir = app_handle
            .path()
            .app_data_dir()
            .map_err(|e| {
                sqlx::Error::Io(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    format!("failed to get app data dir: {}", e),
                ))
            })?;

        let tauri_db_path = app_data_dir.join("meeting_minutes.sqlite");

        Ok(!tauri_db_path.exists())
    }

    /// Import a legacy database from the specified path and initialize
    pub async fn import_legacy_database(
        app_handle: &tauri::AppHandle,
        legacy_db_path: &str,
    ) -> Result<Self> {
        let app_data_dir = app_handle
            .path()
            .app_data_dir()
            .map_err(|e| {
                sqlx::Error::Io(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    format!("failed to get app data dir: {}", e),
                ))
            })?;

        if !app_data_dir.exists() {
            fs::create_dir_all(&app_data_dir).map_err(|e| sqlx::Error::Io(e))?;
        }

        // Copy legacy database to app data directory as meeting_minutes.db
        let target_legacy_path = app_data_dir.join("meeting_minutes.db");
        log::info!(
            "Copying legacy database from {} to {}",
            legacy_db_path,
            target_legacy_path.display()
        );

        fs::copy(legacy_db_path, &target_legacy_path).map_err(|e| sqlx::Error::Io(e))?;

        // Now use the standard initialization which will detect and migrate the legacy db
        Self::new_from_app_handle(app_handle).await
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    pub async fn with_transaction<T, F, Fut>(&self, f: F) -> Result<T>
    where
        F: FnOnce(&mut Transaction<'_, Sqlite>) -> Fut,
        Fut: std::future::Future<Output = Result<T>>,
    {
        let mut tx = self.pool.begin().await?;
        let result = f(&mut tx).await;

        match result {
            Ok(val) => {
                tx.commit().await?;
                Ok(val)
            }
            Err(err) => {
                tx.rollback().await?;
                Err(err)
            }
        }
    }

    /// Cleanup database connection and checkpoint WAL
    /// This should be called on application shutdown to ensure:
    /// - All WAL changes are written to the main database file
    /// - The .wal and .shm files are deleted
    /// - Connection pool is gracefully closed
    pub async fn cleanup(&self) -> Result<()> {
        log::info!("Starting database cleanup...");

        // Force checkpoint of WAL to main database file and remove WAL file
        // TRUNCATE mode: checkpoints all pages AND deletes the WAL file
        match sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
            .execute(&self.pool)
            .await
        {
            Ok(_) => log::info!("WAL checkpoint completed successfully"),
            Err(e) => log::warn!("WAL checkpoint failed (non-fatal): {}", e),
        }

        // Close the connection pool gracefully
        self.pool.close().await;
        log::info!("Database connection pool closed");

        Ok(())
    }
}

/// Reconcile stored migration checksums against the current migration files.
/// Any applied migration whose file content has changed will have its stored checksum
/// updated so sqlx does not refuse to run subsequent migrations.
/// This is safe because the migration itself is not re-run — only the stored checksum
/// is corrected so sqlx can accept the current file as the authoritative version.
async fn reconcile_migration_checksums(
    pool: &SqlitePool,
    migrator: &sqlx::migrate::Migrator,
) -> Result<()> {
    let has_table: Option<i64> = sqlx::query_scalar(
        "SELECT 1 FROM sqlite_master WHERE type='table' AND name='_sqlx_migrations' LIMIT 1",
    )
    .fetch_optional(pool)
    .await?;

    if has_table.is_none() {
        return Ok(());
    }

    let rows = sqlx::query("SELECT version, checksum FROM _sqlx_migrations")
        .fetch_all(pool)
        .await?;

    for row in rows {
        let version: i64 = row.try_get("version")?;
        let db_checksum: Vec<u8> = row.try_get("checksum")?;
        if let Some(migration) = migrator.iter().find(|m| m.version == version) {
            if db_checksum != migration.checksum.as_ref() {
                log::warn!(
                    "Reconciling migration checksum for version {} to match current file",
                    version
                );
                sqlx::query("UPDATE _sqlx_migrations SET checksum = ? WHERE version = ?")
                    .bind(migration.checksum.as_ref())
                    .bind(version)
                    .execute(pool)
                    .await?;
            }
        }
    }

    Ok(())
}

/// Run pending migrations with recovery for known safe failure modes:
///
/// 1. If a migration fails because a column already exists (an `ensure_*` helper added
///    it in a previous run), we mark the migration as applied and continue.
/// 2. If a migration fails for any other reason the error is propagated normally.
async fn run_migrations_with_recovery(
    pool: &SqlitePool,
    migrator: &sqlx::migrate::Migrator,
) -> Result<()> {
    // First attempt — the common case.
    match migrator.run(pool).await {
        Ok(_) => return Ok(()),
        Err(e) => {
            let err_str = e.to_string().to_lowercase();
            if !err_str.contains("already has a column named")
                && !err_str.contains("duplicate column")
            {
                // Not a duplicate-column error; propagate as-is.
                return Err(e.into());
            }
            log::warn!(
                "Migration failed with duplicate column error: {}. \
                 A column was likely added by an ensure_* helper before the migration ran. \
                 Attempting recovery.",
                e
            );
        }
    }

    // Recovery pass: walk every migration in order. For ones that are already applied
    // (present in _sqlx_migrations with success = 1) skip them. For the first unapplied
    // migration that would fail with "duplicate column", mark it as applied and continue
    // so the migrator can proceed with the remaining migrations on the next call.
    for migration in migrator.iter() {
        let applied: Option<bool> = sqlx::query_scalar(
            "SELECT success FROM _sqlx_migrations WHERE version = ? LIMIT 1",
        )
        .bind(migration.version)
        .fetch_optional(pool)
        .await?;

        if matches!(applied, Some(true)) {
            continue;
        }

        // Check whether this migration only adds a column that already exists.
        let sql_str: &str = &migration.sql;
        let sql_lower = sql_str.to_lowercase();
        if !sql_lower.contains("add column") {
            // Not a column-addition migration; stop here and let the normal
            // migrator error surface for this migration.
            break;
        }

        // Extract the table name and column name from the SQL so we can check
        // whether the column already exists.
        // Expected pattern: ALTER TABLE <table> ADD COLUMN <column> ...
        let column_already_exists = if let Some(col_name) = parse_add_column_name(sql_str) {
            if let Some(table_name) = parse_alter_table_name(sql_str) {
                let exists: Option<i64> = sqlx::query_scalar(&format!(
                    "SELECT 1 FROM pragma_table_info('{}') WHERE name = '{}' LIMIT 1",
                    table_name, col_name
                ))
                .fetch_optional(pool)
                .await
                .unwrap_or(None);
                exists.is_some()
            } else {
                false
            }
        } else {
            false
        };

        if column_already_exists {
            log::warn!(
                "Migration {} would fail because its column already exists. \
                 Marking as applied so remaining migrations can proceed.",
                migration.version
            );
            sqlx::query(
                "INSERT OR REPLACE INTO _sqlx_migrations \
                 (version, description, installed_on, success, checksum, execution_time) \
                 VALUES (?, ?, CURRENT_TIMESTAMP, 1, ?, 0)",
            )
            .bind(migration.version)
            .bind(migration.description.as_ref())
            .bind(migration.checksum.as_ref())
            .execute(pool)
            .await?;
        } else {
            // The column doesn't exist yet, so the migration should run normally.
            // Stop recovery here; the normal migrator will handle it on the next call.
            break;
        }
    }

    // Retry — remaining unapplied migrations should now succeed.
    migrator.run(pool).await.map_err(Into::into)
}

/// Parse the table name from `ALTER TABLE <name> ADD COLUMN ...`
fn parse_alter_table_name(sql: &str) -> Option<&str> {
    let lower = sql.to_lowercase();
    let after_alter = lower.find("alter table")?.checked_add("alter table".len())?;
    let rest = sql[after_alter..].trim();
    let end = rest.find(|c: char| c.is_whitespace())?;
    Some(rest[..end].trim())
}

/// Parse the column name from `ALTER TABLE <name> ADD COLUMN <col> ...`
fn parse_add_column_name(sql: &str) -> Option<&str> {
    let lower = sql.to_lowercase();
    let after_add = lower.find("add column")?.checked_add("add column".len())?;
    let rest = sql[after_add..].trim();
    let end = rest.find(|c: char| c.is_whitespace())?;
    Some(rest[..end].trim())
}
