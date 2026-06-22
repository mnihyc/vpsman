export const DEFAULT_MAX_JOB_TIMEOUT_SECS = 3600;
export const MAX_CONFIGURABLE_JOB_TIMEOUT_SECS = 7 * 24 * 60 * 60;

export function clampJobMaxTimeoutSecs(value: number): number {
  if (!Number.isFinite(value)) {
    return 1;
  }
  return Math.trunc(Math.min(Math.max(value, 1), MAX_CONFIGURABLE_JOB_TIMEOUT_SECS));
}

export function parseOptionalJobMaxTimeoutSecs(value: string): number | undefined {
  const trimmed = value.trim();
  if (!trimmed) {
    return undefined;
  }
  const parsed = Number(trimmed);
  if (!Number.isFinite(parsed)) {
    return undefined;
  }
  return clampJobMaxTimeoutSecs(parsed);
}

export function effectiveJobMaxTimeoutSecs(value: string | number | undefined): number {
  if (typeof value === "number") {
    return clampJobMaxTimeoutSecs(value);
  }
  if (typeof value === "string") {
    return parseOptionalJobMaxTimeoutSecs(value) ?? DEFAULT_MAX_JOB_TIMEOUT_SECS;
  }
  return DEFAULT_MAX_JOB_TIMEOUT_SECS;
}
