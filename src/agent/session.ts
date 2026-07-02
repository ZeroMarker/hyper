import fs from 'node:fs/promises';
import path from 'node:path';
import { nanoid } from 'nanoid';
import type { WorkspacePaths } from '../workspace/paths.js';

export interface SessionMessage {
  role: 'user' | 'assistant' | 'tool' | 'system';
  content: string;
  timestamp: string;
  metadata?: Record<string, unknown>;
}

export interface AgentSession {
  id: string;
  messages: SessionMessage[];
}

export async function createSession(workspace: WorkspacePaths): Promise<AgentSession> {
  await fs.mkdir(workspace.sessionsDir, { recursive: true });
  return { id: nanoid(), messages: [] };
}

export async function saveSession(workspace: WorkspacePaths, session: AgentSession): Promise<void> {
  await fs.mkdir(workspace.sessionsDir, { recursive: true });
  const file = path.join(workspace.sessionsDir, `${session.id}.jsonl`);
  const lines = session.messages.map((message) => JSON.stringify(message)).join('\n');
  await fs.writeFile(file, lines ? `${lines}\n` : '', 'utf8');
}

export function addSessionMessage(
  session: AgentSession,
  role: SessionMessage['role'],
  content: string,
  metadata: Record<string, unknown> = {}
): void {
  session.messages.push({
    role,
    content,
    timestamp: new Date().toISOString(),
    metadata
  });
}
