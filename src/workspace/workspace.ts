import fs from 'node:fs/promises';
import path from 'node:path';
import { resolveRunPaths, resolveWorkspace, type RunPaths, type WorkspacePaths } from './paths.js';
import { HarnessDatabase } from './sqlite.js';

export interface Workspace {
  paths: WorkspacePaths;
  database: HarnessDatabase;
}

export async function initWorkspace(root = process.cwd()): Promise<Workspace> {
  const paths = resolveWorkspace(root);
  await fs.mkdir(paths.runsDir, { recursive: true });
  await fs.mkdir(paths.sessionsDir, { recursive: true });
  const database = new HarnessDatabase(paths.db);
  return { paths, database };
}

export async function openWorkspace(root = process.cwd()): Promise<Workspace> {
  const paths = resolveWorkspace(root);
  try {
    await fs.access(paths.dir);
  } catch {
    return initWorkspace(root);
  }
  return { paths, database: new HarnessDatabase(paths.db) };
}

export async function prepareRun(workspace: Workspace, runId: string): Promise<RunPaths> {
  const runPaths = resolveRunPaths(workspace.paths, runId);
  await fs.mkdir(runPaths.artifactsDir, { recursive: true });
  await fs.mkdir(runPaths.checkpointsDir, { recursive: true });
  return runPaths;
}

export function toProjectPath(root: string, absolutePath: string): string {
  return path.relative(root, absolutePath) || '.';
}
