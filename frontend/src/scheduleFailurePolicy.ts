export const DEFAULT_SCHEDULE_MAX_FAILURES = -1;

export function formatScheduleFailures(
  failureCount: number,
  maxFailures: number,
): string {
  const failures = `${failureCount} failure${failureCount === 1 ? "" : "s"}`;
  return maxFailures === -1
    ? `${failures} · automatic pause off`
    : `${failures} · ${maxFailures} tolerated`;
}
