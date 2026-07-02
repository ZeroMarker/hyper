import { execa } from 'execa';
import type { ToolContext, ToolOutput } from './types.js';

export interface BashInput {
  command: string;
  timeoutMs?: number;
}

export async function bashTool(input: BashInput, context: ToolContext): Promise<ToolOutput> {
  context.policy.assertAllowed({ mode: context.mode, action: 'bash', command: input.command });
  await context.events.write('tool.started', { tool: 'bash', command: input.command }, context.stepId ?? null, context.stepIndex ?? null);

  const startedAt = Date.now();
  const result = await execa(input.command, {
    cwd: context.workspace.root,
    shell: true,
    reject: false,
    timeout: input.timeoutMs ?? 120_000,
    all: true
  });
  const payload = {
    command: input.command,
    cwd: context.workspace.root,
    exitCode: result.exitCode,
    stdout: result.stdout,
    stderr: result.stderr,
    all: result.all,
    durationMs: Date.now() - startedAt
  };

  await context.events.write('tool.finished', { tool: 'bash', ...payload }, context.stepId ?? null, context.stepIndex ?? null);
  return { ok: result.exitCode === 0, payload };
}
