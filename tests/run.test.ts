import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { describe, expect, it } from 'vitest';
import { getRunDetails, runTask } from '../src/agent/loop.js';
import { restoreCheckpoint, type Checkpoint } from '../src/workspace/checkpoints.js';

async function tempRoot() {
  return fs.mkdtemp(path.join(os.tmpdir(), 'harness-'));
}

describe('runTask', () => {
  it('runs a shell step and records events', async () => {
    const root = await tempRoot();
    const result = await runTask({
      name: 'hello',
      steps: [{ id: 'hello', mode: 'build', instruction: 'bash:echo ok', metadata: {} }],
      metadata: {}
    }, { root });

    expect(result.summary.status).toBe('finished');
    const details = await getRunDetails(result.runId, root);
    expect(details.events.map((event) => event.type)).toContain('run.finished');
    expect(details.events.map((event) => event.type)).toContain('tool.finished');
  });

  it('denies writes in plan mode', async () => {
    const root = await tempRoot();
    const result = await runTask({
      name: 'readonly',
      steps: [{ id: 'write', mode: 'plan', instruction: 'write:demo.txt\nnope', metadata: {} }],
      metadata: {}
    }, { root });

    expect(result.summary.status).toBe('failed');
    expect(result.summary.failure?.message).toMatch(/read-only/);
  });

  it('fails the run when a shell step exits non-zero', async () => {
    const root = await tempRoot();
    const result = await runTask({
      name: 'fail-shell',
      steps: [{ id: 'shell', mode: 'build', instruction: 'bash:exit 7', metadata: {} }],
      metadata: {}
    }, { root });

    expect(result.summary.status).toBe('failed');
    expect(result.summary.stepsSucceeded).toBe(0);
    expect(result.summary.failure?.message).toContain('exit code 7');
  });

  it('rejects file access outside the workspace root', async () => {
    const root = await tempRoot();
    const result = await runTask({
      name: 'escape',
      steps: [{ id: 'read', mode: 'build', instruction: 'read:../outside.txt', metadata: {} }],
      metadata: {}
    }, { root });

    expect(result.summary.status).toBe('failed');
    expect(result.summary.failure?.message).toMatch(/escapes workspace root/);
  });

  it('writes a file and can restore its checkpoint', async () => {
    const root = await tempRoot();
    await fs.writeFile(path.join(root, 'demo.txt'), 'before', 'utf8');

    const result = await runTask({
      name: 'write',
      steps: [{ id: 'write', mode: 'build', instruction: 'write:demo.txt\nafter', metadata: {} }],
      metadata: {}
    }, { root });

    expect(await fs.readFile(path.join(root, 'demo.txt'), 'utf8')).toBe('after');
    const checkpointFiles = await fs.readdir(path.join(root, '.harness', 'runs', result.runId, 'checkpoints'));
    const checkpointJson = checkpointFiles.find((file) => file.endsWith('.json'));
    expect(checkpointJson).toBeTruthy();

    const checkpoint = JSON.parse(
      await fs.readFile(path.join(root, '.harness', 'runs', result.runId, 'checkpoints', checkpointJson!), 'utf8')
    ) as Checkpoint;
    await restoreCheckpoint(root, checkpoint);
    expect(await fs.readFile(path.join(root, 'demo.txt'), 'utf8')).toBe('before');
  });
});
