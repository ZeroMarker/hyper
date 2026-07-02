import path from 'node:path';
import type { AgentMode } from '../schemas/task.js';
import { assertInsideRoot } from '../workspace/paths.js';

export type PolicyDecision = 'allow' | 'confirm' | 'deny';

export interface PolicyRequest {
  mode: AgentMode;
  action: 'read' | 'write' | 'bash';
  target?: string;
  command?: string;
}

export interface PolicyResult {
  decision: PolicyDecision;
  reason?: string;
}

const dangerousCommandPatterns = [
  /\brm\s+-rf\b/,
  /\bsudo\b/,
  /\bchmod\s+-R\b/,
  /\bchown\s+-R\b/,
  />\s*\/dev\/sd[a-z]/,
  /\bdd\s+/
];

export class PolicyEngine {
  constructor(readonly root: string) {}

  evaluate(request: PolicyRequest): PolicyResult {
    if (request.target) {
      const absolute = path.resolve(this.root, request.target);
      assertInsideRoot(this.root, absolute);
    }

    if (request.action === 'read') {
      return { decision: 'allow' };
    }

    if (request.mode === 'plan' && request.action === 'write') {
      return { decision: 'deny', reason: 'plan mode is read-only' };
    }

    if (request.action === 'bash') {
      const command = request.command ?? '';
      if (request.mode === 'plan') {
        return { decision: 'confirm', reason: 'bash requires confirmation in plan mode' };
      }
      if (dangerousCommandPatterns.some((pattern) => pattern.test(command))) {
        return { decision: 'confirm', reason: 'command matches dangerous pattern' };
      }
    }

    return { decision: 'allow' };
  }

  assertAllowed(request: PolicyRequest): void {
    const result = this.evaluate(request);
    if (result.decision === 'deny') {
      throw new Error(result.reason ?? 'policy denied request');
    }
    if (result.decision === 'confirm') {
      throw new Error(result.reason ?? 'policy requires confirmation');
    }
  }
}
