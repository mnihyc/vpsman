const RAPID_REPEAT_WINDOW_MS = 600;

export type SubmissionGuard = {
  lastSuccessfulAt: number;
  lastSuccessfulKey: string | null;
  pendingKey: string | null;
};

export function createSubmissionGuard(): SubmissionGuard {
  return {
    lastSuccessfulAt: 0,
    lastSuccessfulKey: null,
    pendingKey: null,
  };
}

export function beginSubmission(
  guard: SubmissionGuard,
  key: string,
  now = Date.now(),
): boolean {
  if (guard.pendingKey !== null) return false;
  if (
    guard.lastSuccessfulKey === key &&
    now - guard.lastSuccessfulAt < RAPID_REPEAT_WINDOW_MS
  ) {
    return false;
  }
  guard.pendingKey = key;
  return true;
}

export function finishSubmission(
  guard: SubmissionGuard,
  key: string,
  successful: boolean,
  now = Date.now(),
) {
  if (guard.pendingKey === key) guard.pendingKey = null;
  if (!successful) return;
  guard.lastSuccessfulKey = key;
  guard.lastSuccessfulAt = now;
}
