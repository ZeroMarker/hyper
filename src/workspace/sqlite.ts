import Database from 'better-sqlite3';
import type { HarnessEvent } from '../schemas/event.js';
import type { TaskSpec } from '../schemas/task.js';

export interface RunRow {
  runId: string;
  taskId: string;
  taskName: string;
  status: string;
  startedAt: string;
  finishedAt: string | null;
}

export class HarnessDatabase {
  readonly db: Database.Database;

  constructor(path: string) {
    this.db = new Database(path);
    this.db.exec(`
      CREATE TABLE IF NOT EXISTS runs (
        run_id TEXT PRIMARY KEY,
        task_id TEXT NOT NULL,
        task_name TEXT NOT NULL,
        status TEXT NOT NULL,
        started_at TEXT NOT NULL,
        finished_at TEXT
      );

      CREATE TABLE IF NOT EXISTS events (
        event_id TEXT PRIMARY KEY,
        run_id TEXT NOT NULL,
        task_id TEXT NOT NULL,
        type TEXT NOT NULL,
        timestamp TEXT NOT NULL,
        step_id TEXT,
        step_index INTEGER,
        payload_json TEXT NOT NULL
      );

      CREATE INDEX IF NOT EXISTS idx_events_run_id ON events(run_id, timestamp);
      CREATE INDEX IF NOT EXISTS idx_runs_started_at ON runs(started_at);
    `);
  }

  close(): void {
    this.db.close();
  }

  createRun(runId: string, task: TaskSpec, timestamp: string): void {
    const taskId = task.id ?? task.name;
    this.db.prepare(`
      INSERT INTO runs (run_id, task_id, task_name, status, started_at)
      VALUES (?, ?, ?, 'running', ?)
    `).run(runId, taskId, task.name, timestamp);
  }

  insertEvent(event: HarnessEvent): void {
    this.db.prepare(`
      INSERT OR REPLACE INTO events (
        event_id, run_id, task_id, type, timestamp, step_id, step_index, payload_json
      ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)
    `).run(
      event.eventId,
      event.runId,
      event.taskId,
      event.type,
      event.timestamp,
      event.stepId,
      event.stepIndex,
      JSON.stringify(event.payload)
    );

    if (event.type === 'run.finished') {
      this.updateRunStatus(event.runId, 'finished', event.timestamp);
    } else if (event.type === 'run.failed') {
      this.updateRunStatus(event.runId, 'failed', event.timestamp);
    }
  }

  updateRunStatus(runId: string, status: string, finishedAt: string): void {
    this.db.prepare(`
      UPDATE runs SET status = ?, finished_at = ? WHERE run_id = ?
    `).run(status, finishedAt, runId);
  }

  listRuns(limit = 20): RunRow[] {
    return this.db.prepare(`
      SELECT
        run_id AS runId,
        task_id AS taskId,
        task_name AS taskName,
        status,
        started_at AS startedAt,
        finished_at AS finishedAt
      FROM runs
      ORDER BY started_at DESC
      LIMIT ?
    `).all(limit) as unknown as RunRow[];
  }

  getRun(runId: string): RunRow | null {
    const row = this.db.prepare(`
      SELECT
        run_id AS runId,
        task_id AS taskId,
        task_name AS taskName,
        status,
        started_at AS startedAt,
        finished_at AS finishedAt
      FROM runs
      WHERE run_id = ?
    `).get(runId) as RunRow | undefined;
    return row ?? null;
  }

  listEvents(runId: string): HarnessEvent[] {
    const rows = this.db.prepare(`
      SELECT
        event_id AS eventId,
        run_id AS runId,
        task_id AS taskId,
        type,
        timestamp,
        step_id AS stepId,
        step_index AS stepIndex,
        payload_json AS payloadJson
      FROM events
      WHERE run_id = ?
      ORDER BY timestamp ASC
    `).all(runId) as Array<Omit<HarnessEvent, 'payload'> & { payloadJson: string }>;

    return rows.map((row) => ({
      eventId: row.eventId,
      runId: row.runId,
      taskId: row.taskId,
      type: row.type,
      timestamp: row.timestamp,
      stepId: row.stepId,
      stepIndex: row.stepIndex,
      payload: JSON.parse(row.payloadJson)
    }));
  }
}
