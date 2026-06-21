export const DEFAULT_MAX_COMMAND_TIMEOUT_SECS = 3600;
export const MAX_CONFIGURABLE_COMMAND_TIMEOUT_SECS = 7 * 24 * 60 * 60;

export function clampCommandTimeoutSecs(value: number): number {
  if (!Number.isFinite(value)) {
    return 1;
  }
  return Math.trunc(Math.min(Math.max(value, 1), MAX_CONFIGURABLE_COMMAND_TIMEOUT_SECS));
}
