use crate::models::{
    AlertState, FaultKind, GridSample, IncidentRecord, ManualOverride, MatchMarker, MetricSource,
    MetricStatus, Platform, ShowPreset,
};
use anyhow::Context;
use chrono::{DateTime, Utc};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::{Row, SqlitePool};
use std::collections::HashMap;
use std::path::Path;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, Notify};

#[derive(Debug, Clone)]
pub enum DbCommand {
    UpsertGridSample(GridSample),
    UpsertIncident(IncidentRecord),
    AcknowledgeIncident(String),
    UpsertMatch(MatchMarker),
    UpsertOverride(ManualOverride),
    SetGlobalIntermission(bool),
    SetGroupIntermission { group_id: String, active: bool },
    Flush(Arc<Notify>),
}

#[derive(Clone)]
pub struct DbHandle {
    pub pool: SqlitePool,
    writer_tx: mpsc::Sender<DbCommand>,
}

impl DbHandle {
    pub async fn open(db_path: &Path, batch_interval_ms: u64) -> anyhow::Result<Self> {
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let opts = SqliteConnectOptions::from_str(&format!("sqlite://{}", db_path.display()))?
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal)
            .busy_timeout(Duration::from_secs(5));

        let pool = SqlitePoolOptions::new()
            .max_connections(4)
            .connect_with(opts)
            .await
            .context("failed to open SQLite WAL pool")?;

        run_migrations(&pool).await?;

        let (writer_tx, writer_rx) = mpsc::channel::<DbCommand>(2048);
        let writer_pool = pool.clone();
        tokio::spawn(async move {
            db_writer_actor(writer_pool, writer_rx, batch_interval_ms).await;
        });

        Ok(Self { pool, writer_tx })
    }

    pub async fn enqueue(&self, cmd: DbCommand) {
        if let Err(err) = self.writer_tx.send(cmd).await {
            tracing::warn!("DB writer channel closed: {err}");
        }
    }

    pub async fn load_grid_samples(&self, show_start: Option<DateTime<Utc>>) -> anyhow::Result<Vec<GridSample>> {
        let rows = if let Some(since) = show_start {
            sqlx::query(
                "SELECT bucket_utc, channel_id, group_id, platform, ccv, ccv_status, source, views, likes, chat_msgs, shares, gifts, coverage_pct, extras_json
                 FROM grid_samples WHERE bucket_utc >= ?1 ORDER BY bucket_utc ASC",
            )
            .bind(since.to_rfc3339())
            .fetch_all(&self.pool)
            .await?
        } else {
            sqlx::query(
                "SELECT bucket_utc, channel_id, group_id, platform, ccv, ccv_status, source, views, likes, chat_msgs, shares, gifts, coverage_pct, extras_json
                 FROM grid_samples ORDER BY bucket_utc ASC",
            )
            .fetch_all(&self.pool)
            .await?
        };

        let mut samples = Vec::with_capacity(rows.len());
        for row in rows {
            let bucket_str: String = row.try_get("bucket_utc")?;
            let platform_str: String = row.try_get("platform")?;
            let status_str: String = row.try_get("ccv_status")?;
            let source_str: String = row.try_get("source")?;
            let extras_raw: String = row.try_get("extras_json")?;
            let bucket_utc = DateTime::parse_from_rfc3339(&bucket_str)
                .map(|dt| dt.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now());
            let platform: Platform = serde_json::from_str(&format!("\"{platform_str}\""))
                .unwrap_or(Platform::Custom);
            let ccv_status: MetricStatus = serde_json::from_str(&format!("\"{status_str}\""))
                .unwrap_or(MetricStatus::Unavailable);
            let source: MetricSource = serde_json::from_str(&format!("\"{source_str}\""))
                .unwrap_or(MetricSource::AutoViewer);
            let extras_json: HashMap<String, serde_json::Value> =
                serde_json::from_str(&extras_raw).unwrap_or_default();

            samples.push(GridSample {
                bucket_utc,
                channel_id: row.try_get("channel_id")?,
                group_id: row.try_get("group_id")?,
                platform,
                ccv: row.try_get("ccv")?,
                ccv_status,
                source,
                views: row.try_get("views")?,
                likes: row.try_get("likes")?,
                chat_msgs: row.try_get("chat_msgs")?,
                shares: row.try_get("shares")?,
                gifts: row.try_get("gifts")?,
                coverage_pct: row.try_get("coverage_pct")?,
                extras_json,
            });
        }

        Ok(samples)
    }

    pub async fn load_incidents(&self) -> anyhow::Result<Vec<IncidentRecord>> {
        let rows = sqlx::query(
            "SELECT id, channel_id, channel_name, group_id, fault, severity, started_at, resolved_at, duration_secs, clip_filename, acknowledged, detail
             FROM incidents ORDER BY started_at DESC LIMIT 500",
        )
        .fetch_all(&self.pool)
        .await?;

        let mut list = Vec::with_capacity(rows.len());
        for row in rows {
            let fault_s: String = row.try_get("fault")?;
            let sev_s: String = row.try_get("severity")?;
            let started_s: String = row.try_get("started_at")?;
            let resolved_s: Option<String> = row.try_get("resolved_at")?;

            let fault: FaultKind = serde_json::from_str(&format!("\"{fault_s}\""))
                .unwrap_or(FaultKind::StreamStall);
            let severity: AlertState = serde_json::from_str(&format!("\"{sev_s}\""))
                .unwrap_or(AlertState::Warning);
            let started_at = DateTime::parse_from_rfc3339(&started_s)
                .map(|d| d.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now());
            let resolved_at = resolved_s.and_then(|s| {
                DateTime::parse_from_rfc3339(&s)
                    .ok()
                    .map(|d| d.with_timezone(&Utc))
            });

            list.push(IncidentRecord {
                id: row.try_get("id")?,
                channel_id: row.try_get("channel_id")?,
                channel_name: row.try_get("channel_name")?,
                group_id: row.try_get("group_id")?,
                fault,
                severity,
                started_at,
                resolved_at,
                duration_secs: row.try_get("duration_secs")?,
                clip_filename: row.try_get("clip_filename")?,
                acknowledged: row.try_get::<i64, _>("acknowledged")? != 0,
                detail: row.try_get("detail")?,
            });
        }

        Ok(list)
    }

    pub async fn load_matches(&self) -> anyhow::Result<Vec<MatchMarker>> {
        let rows = sqlx::query(
            "SELECT id, show_id, label, started_at, ended_at FROM matches ORDER BY started_at ASC",
        )
        .fetch_all(&self.pool)
        .await?;

        let mut matches = Vec::with_capacity(rows.len());
        for row in rows {
            let started_s: String = row.try_get("started_at")?;
            let ended_s: Option<String> = row.try_get("ended_at")?;
            matches.push(MatchMarker {
                id: row.try_get("id")?,
                show_id: row.try_get("show_id")?,
                label: row.try_get("label")?,
                started_at: DateTime::parse_from_rfc3339(&started_s)
                    .map(|d| d.with_timezone(&Utc))
                    .unwrap_or_else(|_| Utc::now()),
                ended_at: ended_s.and_then(|s| {
                    DateTime::parse_from_rfc3339(&s)
                        .ok()
                        .map(|d| d.with_timezone(&Utc))
                }),
            });
        }
        Ok(matches)
    }

    pub async fn load_overrides(&self) -> anyhow::Result<Vec<ManualOverride>> {
        let rows = sqlx::query(
            "SELECT id, channel_id, metric_name, bucket_utc, value, operator_note, updated_at FROM manual_overrides ORDER BY updated_at DESC",
        )
        .fetch_all(&self.pool)
        .await?;

        let mut items = Vec::with_capacity(rows.len());
        for row in rows {
            let bucket_s: Option<String> = row.try_get("bucket_utc")?;
            let updated_s: String = row.try_get("updated_at")?;
            items.push(ManualOverride {
                id: row.try_get("id")?,
                channel_id: row.try_get("channel_id")?,
                metric_name: row.try_get("metric_name")?,
                bucket_utc: bucket_s.and_then(|s| {
                    DateTime::parse_from_rfc3339(&s)
                        .ok()
                        .map(|d| d.with_timezone(&Utc))
                }),
                value: row.try_get("value")?,
                operator_note: row.try_get("operator_note")?,
                updated_at: DateTime::parse_from_rfc3339(&updated_s)
                    .map(|d| d.with_timezone(&Utc))
                    .unwrap_or_else(|_| Utc::now()),
            });
        }
        Ok(items)
    }

    pub async fn upsert_show_now(&self, show: &ShowPreset) -> anyhow::Result<()> {
        let json = serde_json::to_string(show)?;
        sqlx::query(
            "INSERT INTO shows (show_id, title, preset_json, updated_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(show_id) DO UPDATE SET title=excluded.title, preset_json=excluded.preset_json, updated_at=excluded.updated_at",
        )
        .bind(&show.show_id)
        .bind(&show.title)
        .bind(json)
        .bind(Utc::now().to_rfc3339())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn reset_accounting_now(&self) -> anyhow::Result<()> {
        self.flush_pending().await;
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM grid_samples").execute(&mut *tx).await?;
        sqlx::query("DELETE FROM matches").execute(&mut *tx).await?;
        sqlx::query("DELETE FROM manual_overrides").execute(&mut *tx).await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn flush_pending(&self) {
        let notify = Arc::new(Notify::new());
        if self.writer_tx.send(DbCommand::Flush(Arc::clone(&notify))).await.is_ok() {
            notify.notified().await;
        }
    }

    pub async fn clear_clips_now(&self) -> anyhow::Result<u64> {
        self.flush_pending().await;
        let res = sqlx::query("UPDATE incidents SET clip_filename = NULL WHERE clip_filename IS NOT NULL")
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected())
    }

    pub async fn clear_clip_filename_now(&self, filename: &str) -> anyhow::Result<u64> {
        self.flush_pending().await;
        let res = sqlx::query("UPDATE incidents SET clip_filename = NULL WHERE clip_filename = ?1")
            .bind(filename)
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected())
    }

    pub async fn clear_resolved_incidents_now(&self) -> anyhow::Result<u64> {
        self.flush_pending().await;
        let res = sqlx::query("DELETE FROM incidents WHERE resolved_at IS NOT NULL")
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected())
    }

    /// Close any lingering `resolved_at IS NULL` incidents left behind by a prior process exit.
    pub async fn close_stale_open_incidents_now(&self) -> anyhow::Result<u64> {
        self.flush_pending().await;
        let now = Utc::now().to_rfc3339();
        let res = sqlx::query(
            "UPDATE incidents SET resolved_at = ?1, acknowledged = 1 WHERE resolved_at IS NULL",
        )
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(res.rows_affected())
    }

    pub async fn clear_all_incidents_now(&self) -> anyhow::Result<u64> {
        self.flush_pending().await;
        let res = sqlx::query("DELETE FROM incidents")
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected())
    }

    pub async fn load_latest_show_with_ts(&self) -> anyhow::Result<Option<(ShowPreset, DateTime<Utc>)>> {
        let row = sqlx::query("SELECT preset_json, updated_at FROM shows ORDER BY updated_at DESC LIMIT 1")
            .fetch_optional(&self.pool)
            .await?;
        if let Some(r) = row {
            let raw: String = r.try_get("preset_json")?;
            let updated_s: String = r.try_get("updated_at")?;
            let preset: ShowPreset = serde_json::from_str(&raw)?;
            let updated_at = DateTime::parse_from_rfc3339(&updated_s)
                .map(|d| d.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now());
            Ok(Some((preset, updated_at)))
        } else {
            Ok(None)
        }
    }

    pub async fn load_latest_show(&self) -> anyhow::Result<Option<ShowPreset>> {
        Ok(self.load_latest_show_with_ts().await?.map(|(p, _)| p))
    }

    pub async fn list_shows_now(&self) -> anyhow::Result<Vec<(ShowPreset, DateTime<Utc>)>> {
        let rows = sqlx::query(
            "SELECT preset_json, updated_at FROM shows ORDER BY updated_at DESC",
        )
        .fetch_all(&self.pool)
        .await?;

        let mut list = Vec::with_capacity(rows.len());
        for row in rows {
            let raw: String = row.try_get("preset_json")?;
            let updated_s: String = row.try_get("updated_at")?;
            let Ok(preset) = serde_json::from_str::<ShowPreset>(&raw) else {
                continue;
            };
            let updated_at = DateTime::parse_from_rfc3339(&updated_s)
                .map(|d| d.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now());
            list.push((preset, updated_at));
        }
        Ok(list)
    }

    pub async fn get_show_now(&self, show_id: &str) -> anyhow::Result<Option<ShowPreset>> {
        let row = sqlx::query("SELECT preset_json FROM shows WHERE show_id = ?1")
            .bind(show_id)
            .fetch_optional(&self.pool)
            .await?;
        if let Some(r) = row {
            let raw: String = r.try_get("preset_json")?;
            Ok(Some(serde_json::from_str(&raw)?))
        } else {
            Ok(None)
        }
    }

    pub async fn delete_show_now(&self, show_id: &str) -> anyhow::Result<bool> {
        let res = sqlx::query("DELETE FROM shows WHERE show_id = ?1")
            .bind(show_id)
            .execute(&self.pool)
            .await?;
        Ok(res.rows_affected() > 0)
    }

    pub async fn set_active_show_id_now(&self, show_id: &str) -> anyhow::Result<()> {
        sqlx::query(
            "INSERT INTO app_state (key, value) VALUES ('active_show_id', ?1)
             ON CONFLICT(key) DO UPDATE SET value=excluded.value",
        )
        .bind(show_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn load_active_show_with_ts(&self) -> anyhow::Result<Option<(ShowPreset, DateTime<Utc>)>> {
        if let Some(row) = sqlx::query("SELECT value FROM app_state WHERE key = 'active_show_id'")
            .fetch_optional(&self.pool)
            .await?
        {
            let active_id: String = row.try_get("value")?;
            if let Some(show_row) = sqlx::query(
                "SELECT preset_json, updated_at FROM shows WHERE show_id = ?1 LIMIT 1",
            )
            .bind(&active_id)
            .fetch_optional(&self.pool)
            .await?
            {
                let raw: String = show_row.try_get("preset_json")?;
                let updated_s: String = show_row.try_get("updated_at")?;
                if let Ok(preset) = serde_json::from_str::<ShowPreset>(&raw) {
                    let updated_at = DateTime::parse_from_rfc3339(&updated_s)
                        .map(|d| d.with_timezone(&Utc))
                        .unwrap_or_else(|_| Utc::now());
                    return Ok(Some((preset, updated_at)));
                }
            }
        }
        self.load_latest_show_with_ts().await
    }
}

async fn run_migrations(pool: &SqlitePool) -> anyhow::Result<()> {
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS shows (
            show_id TEXT PRIMARY KEY,
            title TEXT NOT NULL,
            preset_json TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS grid_samples (
            bucket_utc TEXT NOT NULL,
            channel_id TEXT NOT NULL,
            group_id TEXT NOT NULL,
            platform TEXT NOT NULL,
            ccv INTEGER,
            ccv_status TEXT NOT NULL,
            source TEXT NOT NULL,
            views INTEGER,
            likes INTEGER,
            chat_msgs INTEGER,
            shares INTEGER,
            gifts INTEGER,
            coverage_pct REAL NOT NULL,
            extras_json TEXT NOT NULL,
            PRIMARY KEY (bucket_utc, channel_id)
        );

        CREATE INDEX IF NOT EXISTS idx_grid_samples_channel ON grid_samples (channel_id, bucket_utc);

        CREATE TABLE IF NOT EXISTS incidents (
            id TEXT PRIMARY KEY,
            channel_id TEXT NOT NULL,
            channel_name TEXT NOT NULL,
            group_id TEXT NOT NULL,
            fault TEXT NOT NULL,
            severity TEXT NOT NULL,
            started_at TEXT NOT NULL,
            resolved_at TEXT,
            duration_secs REAL NOT NULL,
            clip_filename TEXT,
            acknowledged INTEGER NOT NULL DEFAULT 0,
            detail TEXT NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_incidents_started ON incidents (started_at DESC);

        CREATE TABLE IF NOT EXISTS matches (
            id TEXT PRIMARY KEY,
            show_id TEXT NOT NULL,
            label TEXT NOT NULL,
            started_at TEXT NOT NULL,
            ended_at TEXT
        );

        CREATE TABLE IF NOT EXISTS manual_overrides (
            id TEXT PRIMARY KEY,
            channel_id TEXT NOT NULL,
            metric_name TEXT NOT NULL,
            bucket_utc TEXT,
            value REAL NOT NULL,
            operator_note TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS app_state (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );
        "#,
    )
    .execute(pool)
    .await?;
    Ok(())
}

async fn db_writer_actor(
    pool: SqlitePool,
    mut rx: mpsc::Receiver<DbCommand>,
    batch_interval_ms: u64,
) {
    let mut interval = tokio::time::interval(Duration::from_millis(batch_interval_ms.max(100)));
    let mut pending = Vec::with_capacity(64);

    loop {
        tokio::select! {
            maybe_cmd = rx.recv() => {
                match maybe_cmd {
                    Some(DbCommand::Flush(notify)) => {
                        if !pending.is_empty() {
                            flush_batch(&pool, &mut pending).await;
                        }
                        notify.notify_one();
                    }
                    Some(cmd) => pending.push(cmd),
                    None => {
                        if !pending.is_empty() {
                            flush_batch(&pool, &mut pending).await;
                        }
                        break;
                    }
                }
            }
            _ = interval.tick() => {
                if !pending.is_empty() {
                    flush_batch(&pool, &mut pending).await;
                }
            }
        }
    }
}

async fn flush_batch(pool: &SqlitePool, pending: &mut Vec<DbCommand>) {
    let mut tx = match pool.begin().await {
        Ok(tx) => tx,
        Err(err) => {
            tracing::error!("Failed to begin SQLite WAL batch commit: {err}");
            return;
        }
    };

    for cmd in pending.drain(..) {
        if let Err(err) = apply_command(&mut tx, cmd).await {
            tracing::warn!("SQLite batch command error: {err}");
        }
    }

    if let Err(err) = tx.commit().await {
        tracing::error!("Failed to commit SQLite WAL batch: {err}");
    }
}

async fn apply_command(tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>, cmd: DbCommand) -> anyhow::Result<()> {
    match cmd {
        DbCommand::UpsertGridSample(s) => {
            let platform_str = serde_json::to_string(&s.platform)?.trim_matches('"').to_string();
            let status_str = serde_json::to_string(&s.ccv_status)?.trim_matches('"').to_string();
            let source_str = serde_json::to_string(&s.source)?.trim_matches('"').to_string();
            let extras = serde_json::to_string(&s.extras_json)?;
            sqlx::query(
                "INSERT INTO grid_samples (bucket_utc, channel_id, group_id, platform, ccv, ccv_status, source, views, likes, chat_msgs, shares, gifts, coverage_pct, extras_json)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
                 ON CONFLICT(bucket_utc, channel_id) DO UPDATE SET
                   ccv=excluded.ccv, ccv_status=excluded.ccv_status, source=excluded.source,
                   views=excluded.views, likes=excluded.likes, chat_msgs=excluded.chat_msgs,
                   shares=excluded.shares, gifts=excluded.gifts, coverage_pct=excluded.coverage_pct,
                   extras_json=excluded.extras_json",
            )
            .bind(s.bucket_utc.to_rfc3339())
            .bind(&s.channel_id)
            .bind(&s.group_id)
            .bind(platform_str)
            .bind(s.ccv)
            .bind(status_str)
            .bind(source_str)
            .bind(s.views)
            .bind(s.likes)
            .bind(s.chat_msgs)
            .bind(s.shares)
            .bind(s.gifts)
            .bind(s.coverage_pct)
            .bind(extras)
            .execute(&mut **tx)
            .await?;
        }
        DbCommand::UpsertIncident(inc) => {
            let fault_str = serde_json::to_string(&inc.fault)?.trim_matches('"').to_string();
            let sev_str = serde_json::to_string(&inc.severity)?.trim_matches('"').to_string();
            sqlx::query(
                "INSERT INTO incidents (id, channel_id, channel_name, group_id, fault, severity, started_at, resolved_at, duration_secs, clip_filename, acknowledged, detail)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
                 ON CONFLICT(id) DO UPDATE SET
                   severity=excluded.severity, resolved_at=excluded.resolved_at,
                   duration_secs=excluded.duration_secs,
                   clip_filename=COALESCE(excluded.clip_filename, incidents.clip_filename),
                   acknowledged=excluded.acknowledged, detail=excluded.detail",
            )
            .bind(&inc.id)
            .bind(&inc.channel_id)
            .bind(&inc.channel_name)
            .bind(&inc.group_id)
            .bind(fault_str)
            .bind(sev_str)
            .bind(inc.started_at.to_rfc3339())
            .bind(inc.resolved_at.map(|d| d.to_rfc3339()))
            .bind(inc.duration_secs)
            .bind(&inc.clip_filename)
            .bind(if inc.acknowledged { 1 } else { 0 })
            .bind(&inc.detail)
            .execute(&mut **tx)
            .await?;
        }
        DbCommand::AcknowledgeIncident(id) => {
            sqlx::query("UPDATE incidents SET acknowledged=1 WHERE id=?1")
                .bind(id)
                .execute(&mut **tx)
                .await?;
        }
        DbCommand::UpsertMatch(m) => {
            sqlx::query(
                "INSERT INTO matches (id, show_id, label, started_at, ended_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(id) DO UPDATE SET label=excluded.label, ended_at=excluded.ended_at",
            )
            .bind(&m.id)
            .bind(&m.show_id)
            .bind(&m.label)
            .bind(m.started_at.to_rfc3339())
            .bind(m.ended_at.map(|d| d.to_rfc3339()))
            .execute(&mut **tx)
            .await?;
        }
        DbCommand::UpsertOverride(ov) => {
            sqlx::query(
                "INSERT INTO manual_overrides (id, channel_id, metric_name, bucket_utc, value, operator_note, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(id) DO UPDATE SET value=excluded.value, operator_note=excluded.operator_note, updated_at=excluded.updated_at",
            )
            .bind(&ov.id)
            .bind(&ov.channel_id)
            .bind(&ov.metric_name)
            .bind(ov.bucket_utc.map(|d| d.to_rfc3339()))
            .bind(ov.value)
            .bind(&ov.operator_note)
            .bind(ov.updated_at.to_rfc3339())
            .execute(&mut **tx)
            .await?;
        }
        DbCommand::SetGlobalIntermission(active) => {
            sqlx::query(
                "INSERT INTO app_state (key, value) VALUES ('global_intermission', ?1)
                 ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            )
            .bind(if active { "true" } else { "false" })
            .execute(&mut **tx)
            .await?;
        }
        DbCommand::SetGroupIntermission { group_id, active } => {
            sqlx::query(
                "INSERT INTO app_state (key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            )
            .bind(format!("group_intermission:{group_id}"))
            .bind(if active { "true" } else { "false" })
            .execute(&mut **tx)
            .await?;
        }
        DbCommand::Flush(notify) => {
            notify.notify_one();
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_reset_accounting_now_clears_tables() {
        let tmp_dir = std::env::temp_dir().join(format!("pgmwatch-db-test-{}", uuid::Uuid::new_v4()));
        let db_path = tmp_dir.join("test.sqlite");
        let db = DbHandle::open(&db_path, 200).await.expect("open db");

        sqlx::query(
            "INSERT INTO grid_samples (bucket_utc, channel_id, group_id, platform, ccv, ccv_status, source, coverage_pct, extras_json)
             VALUES ('2026-11-22T00:00:00Z', 'ch-1', 'en', 'youtube', 1200, 'VALUE', 'AUTO_VIEWER', 100.0, '{}')",
        )
        .execute(&db.pool)
        .await
        .expect("insert sample");

        sqlx::query(
            "INSERT INTO matches (id, show_id, label, started_at)
             VALUES ('m-1', 'show-1', 'Game 1', '2026-11-22T00:00:00Z')",
        )
        .execute(&db.pool)
        .await
        .expect("insert match");

        sqlx::query(
            "INSERT INTO manual_overrides (id, channel_id, metric_name, bucket_utc, value, operator_note, updated_at)
             VALUES ('ov-1', 'ch-1', 'ccv', '2026-11-22T00:00:00Z', 1500.0, 'fix', '2026-11-22T00:01:00Z')",
        )
        .execute(&db.pool)
        .await
        .expect("insert override");

        db.reset_accounting_now().await.expect("reset accounting");

        assert!(db.load_grid_samples(None).await.unwrap().is_empty());
        assert!(db.load_matches().await.unwrap().is_empty());
        assert!(db.load_overrides().await.unwrap().is_empty());

        let _ = std::fs::remove_dir_all(&tmp_dir);
    }

    #[tokio::test]
    async fn test_clear_clips_and_resolved_incidents_flush_pending_writes() {
        let tmp_dir = std::env::temp_dir().join(format!("pgmwatch-db-clips-{}", uuid::Uuid::new_v4()));
        let db_path = tmp_dir.join("test.sqlite");
        let db = DbHandle::open(&db_path, 5000).await.expect("open db");

        let active_inc = IncidentRecord {
            id: "inc-open".into(),
            channel_id: "ch-1".into(),
            channel_name: "CH 1".into(),
            group_id: "g1".into(),
            fault: FaultKind::StreamStall,
            severity: AlertState::Critical,
            started_at: Utc::now(),
            resolved_at: None,
            duration_secs: 10.0,
            clip_filename: Some("ch-1_open.mp4".into()),
            acknowledged: false,
            detail: "open".into(),
        };
        let resolved_inc = IncidentRecord {
            id: "inc-done".into(),
            channel_id: "ch-1".into(),
            channel_name: "CH 1".into(),
            group_id: "g1".into(),
            fault: FaultKind::StreamStall,
            severity: AlertState::Warning,
            started_at: Utc::now(),
            resolved_at: Some(Utc::now()),
            duration_secs: 5.0,
            clip_filename: Some("ch-1_done.mp4".into()),
            acknowledged: true,
            detail: "resolved".into(),
        };
        db.enqueue(DbCommand::UpsertIncident(active_inc)).await;
        db.enqueue(DbCommand::UpsertIncident(resolved_inc)).await;

        // Clearing clips must flush pending batch first so nulling isn't overwritten.
        assert_eq!(db.clear_clips_now().await.unwrap(), 2);
        let after_clips = db.load_incidents().await.unwrap();
        assert_eq!(after_clips.len(), 2);
        assert!(after_clips.iter().all(|i| i.clip_filename.is_none()));

        assert_eq!(db.clear_resolved_incidents_now().await.unwrap(), 1);
        let after_resolved = db.load_incidents().await.unwrap();
        assert_eq!(after_resolved.len(), 1);
        assert_eq!(after_resolved[0].id, "inc-open");

        // Closing stale open incidents on startup converts prior-session rows into clearable history.
        assert_eq!(db.close_stale_open_incidents_now().await.unwrap(), 1);
        assert_eq!(db.clear_resolved_incidents_now().await.unwrap(), 1);
        assert!(db.load_incidents().await.unwrap().is_empty());

        let _ = std::fs::remove_dir_all(&tmp_dir);
    }

    #[tokio::test]
    async fn test_show_preset_crud_and_active_show_tracking() {
        let tmp_dir = std::env::temp_dir().join(format!("pgmwatch-db-shows-{}", uuid::Uuid::new_v4()));
        let db_path = tmp_dir.join("test.sqlite");
        let db = DbHandle::open(&db_path, 200).await.expect("open db");

        let show_a = ShowPreset {
            show_id: "show-alpha".into(),
            title: "Alpha Event".into(),
            created_at: Utc::now(),
            groups: vec![crate::models::LanguageGroupConfig {
                id: "grp-en".into(),
                name: "English".into(),
                language_code: "EN".into(),
                intermission_active: false,
                complaint_keywords: vec![],
            }],
            channels: vec![],
        };
        let show_b = ShowPreset {
            show_id: "show-beta".into(),
            title: "Beta Event".into(),
            ..show_a.clone()
        };

        db.upsert_show_now(&show_a).await.unwrap();
        db.set_active_show_id_now(&show_a.show_id).await.unwrap();
        // Upserting inactive Show B later must NOT override explicit active_show_id!
        db.upsert_show_now(&show_b).await.unwrap();

        let (active, _) = db.load_active_show_with_ts().await.unwrap().unwrap();
        assert_eq!(active.show_id, "show-alpha");

        let all = db.list_shows_now().await.unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(db.get_show_now("show-beta").await.unwrap().unwrap().title, "Beta Event");
        assert!(db.delete_show_now("show-beta").await.unwrap());
        assert!(!db.delete_show_now("show-beta").await.unwrap());

        let _ = std::fs::remove_dir_all(&tmp_dir);
    }
}
