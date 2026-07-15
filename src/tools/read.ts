import fs from 'node:fs/promises';
import { resolveWorkspacePath } from '../workspace/paths.js';
import type { ToolContext, ToolOutput } from './types.js';

export interface ReadInput {
  path: string;
  maxBytes?: number;
}

export async function readTool(input: ReadInput, context: ToolContext): Promise<ToolOutput> {
  context.policy.assertAllowed({ mode: context.mode, action: 'read', target: input.path });
  await context.events.write('tool.started', { tool: 'read', input }, context.stepId ?? null, context.stepIndex ?? null);

  const absolute = resolveWorkspacePath(context.workspace.root, input.path);
  const maxBytes = input.maxBytes ?? 64_000;
  const buffer = await fs.readFile(absolute);
  const truncated = buffer.byteLength > maxBytes;
  const content = buffer.subarray(0, maxBytes).toString('utf8');
  const payload = { path: input.path, content, truncated, bytes: buffer.byteLength };

  await context.events.write('tool.finished', { tool: 'read', ...payload }, context.stepId ?? null, context.stepIndex ?? null);
  return { ok: true, payload };
}
