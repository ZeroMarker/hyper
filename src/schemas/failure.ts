import { z } from 'zod';

export const failureSchema = z.object({
  errorType: z.string().min(1),
  message: z.string().min(1),
  retryable: z.boolean().default(false),
  stepId: z.string().optional(),
  details: z.record(z.string(), z.unknown()).default({}),
  cause: z.string().optional()
});
export type Failure = z.infer<typeof failureSchema>;

export function failureFromError(error: unknown, stepId?: string): Failure {
  if (error instanceof Error) {
    return {
      errorType: error.name || 'Error',
      message: error.message,
      retryable: false,
      stepId,
      details: {},
      cause: error.stack
    };
  }

  return {
    errorType: 'UnknownError',
    message: String(error),
    retryable: false,
    stepId,
    details: {}
  };
}
