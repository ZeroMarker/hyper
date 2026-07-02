import fs from 'node:fs/promises';
import path from 'node:path';
import { createTwoFilesPatch } from 'diff';
import { createCheckpoint } from '../workspace/checkpoints.js';
import type { ToolContext, ToolOutput } from './types.js';

export interface EditInput {
  path: string;
  search: string;
  replace: string;
}

export async function editTool(input: EditInput, context: ToolContext): Promise<ToolOutput> {
  context.policy.assertAllowed({ mode: context.mode, action: 'write', target: input.path });
  await context.events.write('tool.started', { tool: 'edit', path: input.path }, context.stepId ?? null, context.stepIndex ?? null);

  const absolute = path.resolve(context.workspace.root, input.path);
  const before = await fs.readFile(absolute, 'utf8');
  if (!before.includes(input.search)) {
    throw new Error(`search text not found in ${input.path}`);
  }

  const after = before.replace(input.search, input.replace);
  const checkpoint = await createCheckpoint(context.workspace.root, context.run.checkpointsDir, input.path);
  await fs.writeFile(absolute, after, 'utf8');
  const diff = createTwoFilesPatch(input.path, input.path, before, after);
  const payload = { path: input.path, checkpointId: checkpoint.id, diff };

  await context.events.write('checkpoint.created', { checkpoint }, context.stepId ?? null, context.stepIndex ?? null);
  await context.events.write('tool.finished', { tool: 'edit', ...payload }, context.stepId ?? null, context.stepIndex ?? null);
  return { ok: true, payload };
}
