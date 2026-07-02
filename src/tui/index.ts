import readline from 'node:readline/promises';
import { stdin as input, stdout as output } from 'node:process';
import { promptToTask } from '../agent/adapters.js';
import { listRuns, runTask } from '../agent/loop.js';

export async function runInteractiveTui(): Promise<void> {
  const rl = readline.createInterface({ input, output });
  console.log('Harness TUI');
  console.log('Commands: /help, /runs, /new, /mode plan|build, /quit');

  let mode: 'plan' | 'build' = 'build';

  try {
    while (true) {
      const answer: string = await rl.question(`[${mode}] > `);
      const trimmed: string = answer.trim();
      if (!trimmed) continue;

      if (trimmed === '/quit' || trimmed === '/exit') {
        break;
      }

      if (trimmed === '/help') {
        console.log('/mode plan|build  switch agent mode');
        console.log('/runs             list recent runs');
        console.log('/new              clear screen');
        console.log('/quit             exit');
        console.log('Instruction prefixes: bash:, read:, write:, edit:, search:');
        continue;
      }

      if (trimmed.startsWith('/mode ')) {
        const next: string = trimmed.slice('/mode '.length).trim();
        if (next === 'plan' || next === 'build') {
          mode = next;
          console.log(`mode: ${mode}`);
        } else {
          console.log('usage: /mode plan|build');
        }
        continue;
      }

      if (trimmed === '/runs') {
        const runs = await listRuns();
        for (const run of runs) {
          console.log(`${run.runId}\t${run.status}\t${run.taskName}\t${run.startedAt}`);
        }
        continue;
      }

      if (trimmed === '/new') {
        console.clear();
        continue;
      }

      const result = await runTask(promptToTask(trimmed, mode));
      console.log(`${result.summary.status}: ${result.runId}`);
      if (result.summary.failure) {
        console.log(`${result.summary.failure.errorType}: ${result.summary.failure.message}`);
      }
    }
  } finally {
    rl.close();
  }
}
