import { execa } from 'execa';
import type { ToolContext, ToolOutput } from './types.js';

export interface SearchInput {
  query: string;
  limit?: number;
}

export async function searchTool(input: SearchInput, context: ToolContext): Promise<ToolOutput> {
  context.policy.assertAllowed({ mode: context.mode, action: 'read' });
  await context.events.write('tool.started', { tool: 'search', input }, context.stepId ?? null, context.stepIndex ?? null);

  const result = await execa('rg', ['--line-number', '--fixed-strings', input.query, '.'], {
    cwd: context.workspace.root,
    reject: false,
    timeout: 30_000
  });
  const lines = result.stdout.split('\n').filter(Boolean).slice(0, input.limit ?? 100);
  const payload = { query: input.query, lines, exitCode: result.exitCode };

  await context.events.write('tool.finished', { tool: 'search', ...payload }, context.stepId ?? null, context.stepIndex ?? null);
  return { ok: true, payload };
}
