import { z } from 'zod';

export const agentModeSchema = z.enum(['plan', 'build']);
export type AgentMode = z.infer<typeof agentModeSchema>;

export const stepSchema = z.object({
  id: z.string().min(1),
  mode: agentModeSchema.default('build'),
  instruction: z.string().min(1),
  tools: z.array(z.string()).optional(),
  timeoutMs: z.number().int().positive().optional(),
  metadata: z.record(z.string(), z.unknown()).default({})
});
export type StepSpec = z.infer<typeof stepSchema>;

export const taskSchema = z.object({
  id: z.string().min(1).optional(),
  name: z.string().min(1),
  steps: z.array(stepSchema).min(1),
  metadata: z.record(z.string(), z.unknown()).default({})
}).superRefine((task, ctx) => {
  const seen = new Set<string>();
  for (const [index, step] of task.steps.entries()) {
    if (seen.has(step.id)) {
      ctx.addIssue({
        code: 'custom',
        message: `duplicate step id: ${step.id}`,
        path: ['steps', index, 'id']
      });
    }
    seen.add(step.id);
  }
});
export type TaskSpec = z.infer<typeof taskSchema>;

export function parseTaskSpec(input: unknown): TaskSpec {
  return taskSchema.parse(input);
}
