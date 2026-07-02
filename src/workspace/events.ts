import fs from 'node:fs/promises';
import { nanoid } from 'nanoid';
import type { HarnessEvent, EventType } from '../schemas/event.js';
import type { TaskSpec } from '../schemas/task.js';
import { HarnessDatabase } from './sqlite.js';

export interface EventWriterOptions {
  runId: string;
  task: TaskSpec;
  eventsPath: string;
  database: HarnessDatabase;
}

export class EventWriter {
  readonly runId: string;
  readonly task: TaskSpec;
  readonly eventsPath: string;
  readonly database: HarnessDatabase;

  constructor(options: EventWriterOptions) {
    this.runId = options.runId;
    this.task = options.task;
    this.eventsPath = options.eventsPath;
    this.database = options.database;
  }

  async write(
    type: EventType,
    payload: Record<string, unknown> = {},
    stepId: string | null = null,
    stepIndex: number | null = null
  ): Promise<HarnessEvent> {
    const event: HarnessEvent = {
      eventId: nanoid(),
      runId: this.runId,
      taskId: this.task.id ?? this.task.name,
      type,
      timestamp: new Date().toISOString(),
      stepId,
      stepIndex,
      payload
    };

    await fs.appendFile(this.eventsPath, `${JSON.stringify(event)}\n`, 'utf8');
    this.database.insertEvent(event);
    return event;
  }
}
