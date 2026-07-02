import fs from 'node:fs/promises';
import path from 'node:path';
import { createTwoFilesPatch } from 'diff';
import { createCheckpoint } from '../workspace/checkpoints.js';
import type { ToolContext, ToolOutput } from './types.js';

export interface WriteInput {
  path: string;
  content: string;
}

export async function writeTool(input: WriteInput, context: ToolContext): Promise<ToolOutput> {
  context.policy.assertAllowed({ mode: context.mode, action: 'write', target: input.path });
  await context.events.write('tool.started', { tool: 'write', path: input.path }, context.stepId ?? null, context.stepIndex ?? null);

  const absolute = path.resolve(context.workspace.root, input.path);
  let before = '';
  try {
    before = await fs.readFile(absolute, 'utf8');
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code !== 'ENOENT') {
      throw error;
    }
  }

  const checkpoint = await createCheckpoint(context.workspace.root, context.run.checkpointsDir, input.path);
  await fs.mkdir(path.dirname(absolute), { recursive: true });
  await fs.writeFile(absolute, input.content, 'utf8');
  const diff = createTwoFilesPatch(input.path, input.path, before, input.content);
  const payload = { path: input.path, checkpointId: checkpoint.id, diff };

  await context.events.write('checkpoint.created', { checkpoint }, context.stepId ?? null, context.stepIndex ?? null);
  await context.events.write('tool.finished', { tool: 'write', ...payload }, context.stepId ?? null, context.stepIndex ?? null);
  return { ok: true, payload };
}
