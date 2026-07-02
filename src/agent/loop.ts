import fs from 'node:fs/promises';
import { nanoid } from 'nanoid';
import type { Failure } from '../schemas/failure.js';
import { failureFromError } from '../schemas/failure.js';
import type { TaskSpec, StepSpec } from '../schemas/task.js';
import { PolicyEngine } from '../policy/engine.js';
import { bashTool } from '../tools/bash.js';
import { editTool } from '../tools/edit.js';
import { readTool } from '../tools/read.js';
import { searchTool } from '../tools/search.js';
import type { ToolContext, ToolOutput } from '../tools/types.js';
import { writeTool } from '../tools/write.js';
import { EventWriter } from '../workspace/events.js';
import { openWorkspace, prepareRun, type Workspace } from '../workspace/workspace.js';
import { addSessionMessage, createSession, saveSession, type AgentSession } from './session.js';

export interface RunSummary {
  runId: string;
  taskName: string;
  status: 'finished' | 'failed';
  stepsTotal: number;
  stepsSucceeded: number;
  stepsFailed: number;
  startedAt: string;
  finishedAt: string;
  failure?: Failure;
}

export interface RunResult {
  runId: string;
  summary: RunSummary;
}

export interface RunOptions {
  root?: string;
  session?: AgentSession;
}

export async function runTask(task: TaskSpec, options: RunOptions = {}): Promise<RunResult> {
  const workspace = await openWorkspace(options.root ?? process.cwd());
  try {
    const runId = nanoid();
    const runPaths = await prepareRun(workspace, runId);
    await fs.writeFile(runPaths.task, JSON.stringify(task, null, 2), 'utf8');

    const startedAt = new Date().toISOString();
    workspace.database.createRun(runId, task, startedAt);
    const events = new EventWriter({ runId, task, eventsPath: runPaths.events, database: workspace.database });
    await events.write('run.started', { taskName: task.name });

    const session = options.session ?? await createSession(workspace.paths);
    addSessionMessage(session, 'user', task.name, { runId });

    let stepsSucceeded = 0;
    let failure: Failure | undefined;

    for (const [index, step] of task.steps.entries()) {
      await events.write('step.started', { instruction: step.instruction, mode: step.mode }, step.id, index);
      const context: ToolContext = {
        mode: step.mode,
        workspace: workspace.paths,
        run: runPaths,
        policy: new PolicyEngine(workspace.paths.root),
        events,
        stepId: step.id,
        stepIndex: index
      };

      try {
        const output = await executeInstruction(step, context);
        await events.write('step.finished', { output: output.payload }, step.id, index);
        addSessionMessage(session, 'assistant', `Step ${step.id} completed`, output.payload);
        stepsSucceeded += 1;
      } catch (error) {
        failure = failureFromError(error, step.id);
        await events.write('tool.failed', { failure }, step.id, index);
        await events.write('step.failed', { failure }, step.id, index);
        await events.write('run.failed', { failure });
        break;
      }
    }

    const finishedAt = new Date().toISOString();
    const summary: RunSummary = {
      runId,
      taskName: task.name,
      status: failure ? 'failed' : 'finished',
      stepsTotal: task.steps.length,
      stepsSucceeded,
      stepsFailed: failure ? 1 : 0,
      startedAt,
      finishedAt,
      failure
    };

    if (!failure) {
      await events.write('run.finished', { summary });
    }

    await fs.writeFile(runPaths.summary, JSON.stringify(summary, null, 2), 'utf8');
    await saveSession(workspace.paths, session);
    return { runId, summary };
  } finally {
    workspace.database.close();
  }
}

async function executeInstruction(step: StepSpec, context: ToolContext): Promise<ToolOutput> {
  const instruction = step.instruction.trim();

  if (instruction.startsWith('bash:')) {
    return bashTool({ command: instruction.slice('bash:'.length).trim(), timeoutMs: step.timeoutMs }, context);
  }

  if (instruction.startsWith('read:')) {
    return readTool({ path: instruction.slice('read:'.length).trim() }, context);
  }

  if (instruction.startsWith('search:')) {
    return searchTool({ query: instruction.slice('search:'.length).trim() }, context);
  }

  if (instruction.startsWith('write:')) {
    const [target, ...contentLines] = instruction.slice('write:'.length).split('\n');
    return writeTool({ path: target.trim(), content: contentLines.join('\n') }, context);
  }

  if (instruction.startsWith('edit:')) {
    const [target, search, ...replaceLines] = instruction.slice('edit:'.length).split('\n');
    return editTool({ path: target.trim(), search, replace: replaceLines.join('\n') }, context);
  }

  if (step.mode === 'plan') {
    return searchTool({ query: instruction, limit: 20 }, context);
  }

  return bashTool({ command: instruction, timeoutMs: step.timeoutMs }, context);
}

export async function listRuns(root = process.cwd()) {
  const workspace = await openWorkspace(root);
  try {
    return workspace.database.listRuns();
  } finally {
    workspace.database.close();
  }
}

export async function getRunDetails(runId: string, root = process.cwd()) {
  const workspace: Workspace = await openWorkspace(root);
  try {
    return {
      run: workspace.database.getRun(runId),
      events: workspace.database.listEvents(runId)
    };
  } finally {
    workspace.database.close();
  }
}
