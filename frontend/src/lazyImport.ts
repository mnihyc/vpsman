import { lazy, type ComponentType, type LazyExoticComponent } from "react";

const TRANSIENT_IMPORT_RETRIES = 3;
const TRANSIENT_IMPORT_RETRY_DELAY_MS = 180;
const MODULE_LOAD_TIMEOUT_MS = 15_000;

type RetryTransientModuleLoadOptions = {
  retryDelayMs?: number;
  retries?: number;
  timeoutMs?: number;
};

export function retryableLazy<T extends ComponentType<any>>(
  load: () => Promise<{ default: T }>,
): LazyExoticComponent<T> {
  return lazy(() => retryTransientModuleLoad(load));
}

export async function retryTransientModuleLoad<T>(
  load: () => Promise<T>,
  options: RetryTransientModuleLoadOptions = {},
): Promise<T> {
  const retries = options.retries ?? TRANSIENT_IMPORT_RETRIES;
  const retryDelayMs = options.retryDelayMs ?? TRANSIENT_IMPORT_RETRY_DELAY_MS;
  const timeoutMs = options.timeoutMs ?? MODULE_LOAD_TIMEOUT_MS;
  const deadline = Date.now() + timeoutMs;
  let lastError: unknown;
  for (let attempt = 0; attempt <= retries; attempt += 1) {
    try {
      const remainingMs = deadline - Date.now();
      if (remainingMs <= 0) {
        throw moduleLoadTimeoutError(timeoutMs);
      }
      return await withTimeout(load(), remainingMs, timeoutMs);
    } catch (error) {
      lastError = error;
      if (
        attempt >= retries ||
        !isTransientModuleLoadError(error)
      ) {
        throw error;
      }
      const delayMs = retryDelayMs * (attempt + 1);
      if (Date.now() + delayMs >= deadline) {
        throw moduleLoadTimeoutError(timeoutMs);
      }
      await wait(delayMs);
    }
  }
  throw lastError;
}

export function isTransientModuleLoadError(error: unknown): boolean {
  const message =
    error instanceof Error
      ? `${error.name}: ${error.message}`
      : typeof error === "string"
        ? error
        : String(error);
  return /Workspace module load timed out|Failed to fetch dynamically imported module|Importing a module script failed|Loading chunk \d+ failed|net::ERR_|NetworkError/i.test(
    message,
  );
}

function withTimeout<T>(
  pending: Promise<T>,
  remainingMs: number,
  timeoutMs: number,
): Promise<T> {
  return new Promise((resolve, reject) => {
    const timeout = globalThis.setTimeout(
      () => reject(moduleLoadTimeoutError(timeoutMs)),
      remainingMs,
    );
    pending.then(
      (value) => {
        globalThis.clearTimeout(timeout);
        resolve(value);
      },
      (error) => {
        globalThis.clearTimeout(timeout);
        reject(error);
      },
    );
  });
}

function moduleLoadTimeoutError(timeoutMs: number): Error {
  return new Error(`Workspace module load timed out after ${timeoutMs}ms`);
}

function wait(delayMs: number): Promise<void> {
  return new Promise((resolve) => {
    globalThis.setTimeout(resolve, delayMs);
  });
}
