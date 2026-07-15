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
  const resolvedRoot = path.resolve(root);
  const dir = path.join(resolvedRoot, WORKSPACE_DIR);
  return {
    root: resolvedRoot,
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
  const relative = path.relative(path.resolve(root), path.resolve(target));
  if (relative.startsWith('..') || path.isAbsolute(relative)) {
    throw new Error(`path escapes workspace root: ${target}`);
  }
}

export function resolveWorkspacePath(root: string, targetPath: string): string {
  const absolute = path.resolve(root, targetPath);
  assertInsideRoot(root, absolute);
  return absolute;
}
