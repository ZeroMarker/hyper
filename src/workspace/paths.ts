import path from 'node:path';

export const WORKSPACE_DIR = '.harness';

export interface WorkspacePaths {
  root: string;
  dir: string;
  db: string;
  runsDir: string;
  sessionsDir: string;
}

export interface RunPaths {
  runDir: string;
  events: string;
  task: string;
  summary: string;
  artifactsDir: string;
  checkpointsDir: string;
}

export function resolveWorkspace(root = process.cwd()): WorkspacePaths {
  const dir = path.join(root, WORKSPACE_DIR);
  return {
    root,
    dir,
    db: path.join(dir, 'harness.db'),
    runsDir: path.join(dir, 'runs'),
    sessionsDir: path.join(dir, 'sessions')
  };
}

export function resolveRunPaths(workspace: WorkspacePaths, runId: string): RunPaths {
  const runDir = path.join(workspace.runsDir, runId);
  return {
    runDir,
    events: path.join(runDir, 'events.jsonl'),
    task: path.join(runDir, 'task.json'),
    summary: path.join(runDir, 'summary.json'),
    artifactsDir: path.join(runDir, 'artifacts'),
    checkpointsDir: path.join(runDir, 'checkpoints')
  };
}

export function assertInsideRoot(root: string, target: string): void {
  const relative = path.relative(root, target);
  if (relative.startsWith('..') || path.isAbsolute(relative)) {
    throw new Error(`path escapes workspace root: ${target}`);
  }
}
