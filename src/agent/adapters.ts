import type { TaskSpec } from '../schemas/task.js';

export function promptToTask(prompt: string, mode: 'plan' | 'build' = 'build'): TaskSpec {
  return {
    name: prompt.slice(0, 80) || 'interactive task',
    steps: [
      {
        id: mode,
        mode,
        instruction: mode === 'plan' ? `search:${prompt}` : prompt,
        metadata: {}
      }
    ],
    metadata: { source: 'prompt' }
  };
}
