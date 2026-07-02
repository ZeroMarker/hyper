import type { EventWriter } from '../workspace/events.js';
import type { RunPaths, WorkspacePaths } from '../workspace/paths.js';
import type { AgentMode } from '../schemas/task.js';
import type { PolicyEngine } from '../policy/engine.js';

export interface ToolContext {
  mode: AgentMode;
  workspace: WorkspacePaths;
  run: RunPaths;
  policy: PolicyEngine;
  events: EventWriter;
  stepId?: string;
  stepIndex?: number;
}

export interface ToolOutput {
  ok: boolean;
  payload: Record<string, unknown>;
}
