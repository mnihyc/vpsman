export const DEFAULT_MAX_JOB_TIMEOUT_SECS = 3600;
export const MAX_CONFIGURABLE_JOB_TIMEOUT_SECS = 7 * 24 * 60 * 60;

export function clampJobTimeoutSecs(value: number): number {
  if (!Number.isFinite(value)) {
    return 1;
  }
  return Math.trunc(Math.min(Math.max(value, 1), MAX_CONFIGURABLE_JOB_TIMEOUT_SECS));
}

export function parseOptionalJobTimeoutSecs(value: string): number | undefined {
  const trimmed = value.trim();
  if (!trimmed) {
    return undefined;
  }
  const parsed = Number(trimmed);
  if (!Number.isFinite(parsed)) {
    return undefined;
  }
  return clampJobTimeoutSecs(parsed);
}

export function effectiveJobTimeoutSecs(value: string | number | undefined): number {
  if (typeof value === "number") {
    return clampJobTimeoutSecs(value);
  }
  if (typeof value === "string") {
    return parseOptionalJobTimeoutSecs(value) ?? DEFAULT_MAX_JOB_TIMEOUT_SECS;
  }
  return DEFAULT_MAX_JOB_TIMEOUT_SECS;
}
