import fs from 'node:fs/promises';
import path from 'node:path';
import { nanoid } from 'nanoid';
import { resolveWorkspacePath } from './paths.js';

export interface Checkpoint {
  id: string;
  targetPath: string;
  snapshotPath: string;
  existed: boolean;
  createdAt: string;
}

export async function createCheckpoint(
  root: string,
  checkpointsDir: string,
  targetPath: string
): Promise<Checkpoint> {
  const absoluteTarget = resolveWorkspacePath(root, targetPath);

  await fs.mkdir(checkpointsDir, { recursive: true });
  const id = nanoid();
  const snapshotPath = path.join(checkpointsDir, `${id}.snapshot`);
  let existed = true;

  try {
    await fs.copyFile(absoluteTarget, snapshotPath);
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code !== 'ENOENT') {
      throw error;
    }
    existed = false;
    await fs.writeFile(snapshotPath, '', 'utf8');
  }

  const checkpoint: Checkpoint = {
    id,
    targetPath,
    snapshotPath,
    existed,
    createdAt: new Date().toISOString()
  };
  await fs.writeFile(
    path.join(checkpointsDir, `${id}.json`),
    JSON.stringify(checkpoint, null, 2),
    'utf8'
  );
  return checkpoint;
}

export async function restoreCheckpoint(root: string, checkpoint: Checkpoint): Promise<void> {
  const absoluteTarget = resolveWorkspacePath(root, checkpoint.targetPath);

  if (!checkpoint.existed) {
    await fs.rm(absoluteTarget, { force: true });
    return;
  }

  await fs.mkdir(path.dirname(absoluteTarget), { recursive: true });
  await fs.copyFile(checkpoint.snapshotPath, absoluteTarget);
}
