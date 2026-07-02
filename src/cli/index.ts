#!/usr/bin/env node
import fs from 'node:fs/promises';
import path from 'node:path';
import { Command } from 'commander';
import 'dotenv/config';
import { promptToTask } from '../agent/adapters.js';
import { getRunDetails, listRuns, runTask } from '../agent/loop.js';
import { parseTaskSpec } from '../schemas/task.js';
import { restoreCheckpoint, type Checkpoint } from '../workspace/checkpoints.js';
import { initWorkspace, openWorkspace } from '../workspace/workspace.js';
import { runInteractiveTui } from '../tui/index.js';

const program = new Command();

program
  .name('harness')
  .description('Terminal-first agent harness for local coding workflows')
  .version('0.1.0');

program.command('init')
  .description('initialize .harness workspace')
  .action(async () => {
    const workspace = await initWorkspace();
    workspace.database.close();
    console.log(`initialized ${workspace.paths.dir}`);
  });

program.command('validate')
  .description('validate a task JSON file')
  .argument('<task>', 'path to task JSON')
  .action(async (taskPath: string) => {
    const task = await readTask(taskPath);
    console.log(`valid task: ${task.name} (${task.steps.length} steps)`);
  });

program.command('run')
  .description('run a task JSON file')
  .argument('<task>', 'path to task JSON')
  .action(async (taskPath: string) => {
    const task = await readTask(taskPath);
    const result = await runTask(task);
    console.log(JSON.stringify(result.summary, null, 2));
  });

program.command('plan')
  .description('run a prompt in read-only plan mode')
  .argument('<prompt>', 'task prompt')
  .action(async (prompt: string) => {
    const result = await runTask(promptToTask(prompt, 'plan'));
    console.log(JSON.stringify(result.summary, null, 2));
  });

program.command('build')
  .description('run a prompt in build mode')
  .argument('<prompt>', 'task prompt or bash command')
  .action(async (prompt: string) => {
    const result = await runTask(promptToTask(prompt, 'build'));
    console.log(JSON.stringify(result.summary, null, 2));
  });

program.command('runs')
  .description('list recent runs')
  .option('-n, --limit <number>', 'number of runs to show', '20')
  .action(async (options: { limit: string }) => {
    const workspace = await openWorkspace();
    try {
      const runs = workspace.database.listRuns(Number.parseInt(options.limit, 10));
      for (const run of runs) {
        console.log(`${run.runId}\t${run.status}\t${run.taskName}\t${run.startedAt}`);
      }
    } finally {
      workspace.database.close();
    }
  });

program.command('show')
  .description('show run details')
  .argument('<runId>', 'run id')
  .action(async (runId: string) => {
    const details = await getRunDetails(runId);
    if (!details.run) {
      console.error(`run not found: ${runId}`);
      process.exitCode = 1;
      return;
    }
    console.log(JSON.stringify(details, null, 2));
  });

program.command('tui')
  .description('start interactive terminal UI')
  .action(async () => {
    await runInteractiveTui();
  });

program.command('artifacts')
  .description('print artifacts directory for a run')
  .argument('<runId>', 'run id')
  .action(async (runId: string) => {
    const workspace = await openWorkspace();
    workspace.database.close();
    console.log(path.join(workspace.paths.runsDir, runId, 'artifacts'));
  });

program.command('undo')
  .description('restore the latest checkpoint for a run')
  .argument('<runId>', 'run id')
  .action(async (runId: string) => {
    const workspace = await openWorkspace();
    try {
      const checkpointsDir = path.join(workspace.paths.runsDir, runId, 'checkpoints');
      const files = (await fs.readdir(checkpointsDir))
        .filter((file) => file.endsWith('.json'))
        .sort();
      const latest = files.at(-1);
      if (!latest) {
        console.error(`no checkpoints for run: ${runId}`);
        process.exitCode = 1;
        return;
      }
      const checkpoint = JSON.parse(
        await fs.readFile(path.join(checkpointsDir, latest), 'utf8')
      ) as Checkpoint;
      await restoreCheckpoint(workspace.paths.root, checkpoint);
      console.log(`restored ${checkpoint.targetPath} from checkpoint ${checkpoint.id}`);
    } finally {
      workspace.database.close();
    }
  });

async function readTask(taskPath: string) {
  const content = await fs.readFile(taskPath, 'utf8');
  return parseTaskSpec(JSON.parse(content));
}

program.parseAsync(process.argv).catch((error: unknown) => {
  const message = error instanceof Error ? error.message : String(error);
  console.error(message);
  process.exitCode = 1;
});

export { program };
