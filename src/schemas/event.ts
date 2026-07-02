import { z } from 'zod';
import { failureSchema } from './failure.js';

export const eventTypeSchema = z.enum([
  'run.started',
  'run.finished',
  'run.failed',
  'step.started',
  'step.finished',
  'step.failed',
  'tool.started',
  'tool.finished',
  'tool.failed',
  'approval.requested',
  'approval.resolved',
  'checkpoint.created'
]);
export type EventType = z.infer<typeof eventTypeSchema>;

export const eventSchema = z.object({
  eventId: z.string().min(1),
  runId: z.string().min(1),
  taskId: z.string().min(1),
  type: eventTypeSchema,
  timestamp: z.string().min(1),
  stepId: z.string().nullable().default(null),
  stepIndex: z.number().int().nullable().default(null),
  payload: z.record(z.string(), z.unknown()).default({})
});
export type HarnessEvent = z.infer<typeof eventSchema>;

export const toolResultSchema = z.object({
  tool: z.string().min(1),
  ok: z.boolean(),
  payload: z.record(z.string(), z.unknown()).default({}),
  failure: failureSchema.optional()
});
export type ToolResult = z.infer<typeof toolResultSchema>;
