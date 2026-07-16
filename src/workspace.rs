use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use chrono::Utc;
use rand::{Rng, distr::Alphanumeric};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};

use crate::model::{HarnessEvent, RunRow, TaskSpec};

pub fn id() -> String {
    rand::rng()
        .sample_iter(&Alphanumeric)
        .take(21)
        .map(char::from)
        .collect()
}
pub fn now() -> String {
    Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

#[derive(Clone, Debug)]
pub struct WorkspacePaths {
    pub root: PathBuf,
    pub dir: PathBuf,
    pub db: PathBuf,
    pub runs: PathBuf,
    pub sessions: PathBuf,
}
#[derive(Clone, Debug)]
pub struct RunPaths {
    pub dir: PathBuf,
    pub events: PathBuf,
    pub task: PathBuf,
    pub summary: PathBuf,
    pub artifacts: PathBuf,
    pub checkpoints: PathBuf,
}

pub struct Workspace {
    pub paths: WorkspacePaths,
    pub db: Connection,
}

impl Workspace {
    pub fn open(root: impl AsRef<Path>) -> Result<Self> {
        let root = root
            .as_ref()
            .canonicalize()
            .with_context(|| format!("invalid workspace root: {}", root.as_ref().display()))?;
        let dir = root.join(".harness");
        let paths = WorkspacePaths {
            root,
            db: dir.join("harness.db"),
            runs: dir.join("runs"),
            sessions: dir.join("sessions"),
            dir,
        };
        fs::create_dir_all(&paths.runs)?;
        fs::create_dir_all(&paths.sessions)?;
        let db = Connection::open(&paths.db)?;
        db.execute_batch("CREATE TABLE IF NOT EXISTS runs (run_id TEXT PRIMARY KEY, task_id TEXT NOT NULL, task_name TEXT NOT NULL, status TEXT NOT NULL, started_at TEXT NOT NULL, finished_at TEXT); CREATE TABLE IF NOT EXISTS events (event_id TEXT PRIMARY KEY, run_id TEXT NOT NULL, task_id TEXT NOT NULL, type TEXT NOT NULL, timestamp TEXT NOT NULL, step_id TEXT, step_index INTEGER, payload_json TEXT NOT NULL); CREATE INDEX IF NOT EXISTS idx_events_run_id ON events(run_id,timestamp); CREATE INDEX IF NOT EXISTS idx_runs_started_at ON runs(started_at);")?;
        Ok(Self { paths, db })
    }
    pub fn prepare_run(&self, run_id: &str) -> Result<RunPaths> {
        let dir = self.paths.runs.join(run_id);
        let paths = RunPaths {
            events: dir.join("events.jsonl"),
            task: dir.join("task.json"),
            summary: dir.join("summary.json"),
            artifacts: dir.join("artifacts"),
            checkpoints: dir.join("checkpoints"),
            dir,
        };
        fs::create_dir_all(&paths.artifacts)?;
        fs::create_dir_all(&paths.checkpoints)?;
        Ok(paths)
    }
    pub fn create_run(&self, id: &str, task: &TaskSpec, started: &str) -> Result<()> {
        self.db.execute(
            "INSERT INTO runs VALUES (?1,?2,?3,'running',?4,NULL)",
            params![id, task.task_id(), task.name, started],
        )?;
        Ok(())
    }
    pub fn insert_event(&self, event: &HarnessEvent) -> Result<()> {
        self.db.execute(
            "INSERT OR REPLACE INTO events VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
            params![
                event.event_id,
                event.run_id,
                event.task_id,
                event.event_type,
                event.timestamp,
                event.step_id,
                event.step_index,
                event.payload.to_string()
            ],
        )?;
        let status = match event.event_type.as_str() {
            "run.finished" => Some("finished"),
            "run.failed" => Some("failed"),
            _ => None,
        };
        if let Some(status) = status {
            self.db.execute(
                "UPDATE runs SET status=?1,finished_at=?2 WHERE run_id=?3",
                params![status, event.timestamp, event.run_id],
            )?;
        }
        Ok(())
    }
    pub fn list_runs(&self, limit: usize) -> Result<Vec<RunRow>> {
        let mut stmt = self.db.prepare("SELECT run_id,task_id,task_name,status,started_at,finished_at FROM runs ORDER BY started_at DESC LIMIT ?1")?;
        Ok(stmt
            .query_map([limit], |r| {
                Ok(RunRow {
                    run_id: r.get(0)?,
                    task_id: r.get(1)?,
                    task_name: r.get(2)?,
                    status: r.get(3)?,
                    started_at: r.get(4)?,
                    finished_at: r.get(5)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?)
    }
    pub fn get_run(&self, run_id: &str) -> Result<Option<RunRow>> {
        let mut stmt = self.db.prepare("SELECT run_id,task_id,task_name,status,started_at,finished_at FROM runs WHERE run_id=?1")?;
        let mut rows = stmt.query([run_id])?;
        Ok(rows.next()?.map(|r| RunRow {
            run_id: r.get(0).unwrap(),
            task_id: r.get(1).unwrap(),
            task_name: r.get(2).unwrap(),
            status: r.get(3).unwrap(),
            started_at: r.get(4).unwrap(),
            finished_at: r.get(5).unwrap(),
        }))
    }
    pub fn events(&self, run_id: &str) -> Result<Vec<HarnessEvent>> {
        let mut stmt=self.db.prepare("SELECT event_id,run_id,task_id,type,timestamp,step_id,step_index,payload_json FROM events WHERE run_id=?1 ORDER BY timestamp")?;
        Ok(stmt
            .query_map([run_id], |r| {
                let payload: String = r.get(7)?;
                Ok(HarnessEvent {
                    event_id: r.get(0)?,
                    run_id: r.get(1)?,
                    task_id: r.get(2)?,
                    event_type: r.get(3)?,
                    timestamp: r.get(4)?,
                    step_id: r.get(5)?,
                    step_index: r.get(6)?,
                    payload: serde_json::from_str(&payload).unwrap_or_default(),
                })
            })?
            .collect::<Result<Vec<_>, _>>()?)
    }
}

pub fn resolve_path(root: &Path, target: &str) -> Result<PathBuf> {
    let joined = root.join(target);
    let mut normalized = PathBuf::new();
    for part in joined.components() {
        match part {
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            std::path::Component::CurDir => {}
            p => normalized.push(p.as_os_str()),
        }
    }
    if !normalized.starts_with(root) {
        bail!("path escapes workspace root: {target}")
    }
    Ok(normalized)
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Checkpoint {
    pub id: String,
    pub target_path: String,
    pub snapshot_path: PathBuf,
    pub existed: bool,
    pub created_at: String,
}
pub fn create_checkpoint(root: &Path, dir: &Path, target: &str) -> Result<Checkpoint> {
    let absolute = resolve_path(root, target)?;
    fs::create_dir_all(dir)?;
    let id = id();
    let snapshot = dir.join(format!("{id}.snapshot"));
    let existed = absolute.exists();
    if existed {
        fs::copy(&absolute, &snapshot)?;
    } else {
        fs::write(&snapshot, "")?;
    }
    let cp = Checkpoint {
        id: id.clone(),
        target_path: target.into(),
        snapshot_path: snapshot,
        existed,
        created_at: now(),
    };
    fs::write(
        dir.join(format!("{id}.json")),
        serde_json::to_vec_pretty(&cp)?,
    )?;
    Ok(cp)
}
pub fn restore_checkpoint(root: &Path, cp: &Checkpoint) -> Result<()> {
    let target = resolve_path(root, &cp.target_path)?;
    if !cp.existed {
        if target.exists() {
            fs::remove_file(target)?;
        }
        return Ok(());
    }
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::copy(&cp.snapshot_path, target)?;
    Ok(())
}
