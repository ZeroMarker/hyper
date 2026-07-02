import { describe, expect, it } from 'vitest';
import { parseTaskSpec } from '../src/schemas/task.js';

describe('task schema', () => {
  it('parses a valid task', () => {
    const task = parseTaskSpec({
      name: 'sample',
      steps: [{ id: 'one', instruction: 'bash:echo ok' }]
    });

    expect(task.steps[0].mode).toBe('build');
  });

  it('rejects duplicate step ids', () => {
    expect(() => parseTaskSpec({
      name: 'sample',
      steps: [
        { id: 'one', instruction: 'bash:echo 1' },
        { id: 'one', instruction: 'bash:echo 2' }
      ]
    })).toThrow(/duplicate step id/);
  });
});
