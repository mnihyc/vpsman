import { lazy, type ComponentType, type LazyExoticComponent } from "react";

const TRANSIENT_IMPORT_RETRIES = 3;
const TRANSIENT_IMPORT_RETRY_DELAY_MS = 180;

export function retryableLazy<T extends ComponentType<any>>(
  load: () => Promise<{ default: T }>,
): LazyExoticComponent<T> {
  return lazy(() => retryTransientModuleLoad(load));
}

export async function retryTransientModuleLoad<T>(
  load: () => Promise<T>,
): Promise<T> {
  let lastError: unknown;
  for (let attempt = 0; attempt <= TRANSIENT_IMPORT_RETRIES; attempt += 1) {
    try {
      return await load();
    } catch (error) {
      lastError = error;
      if (
        attempt >= TRANSIENT_IMPORT_RETRIES ||
        !isTransientModuleLoadError(error)
      ) {
        throw error;
      }
      await wait(TRANSIENT_IMPORT_RETRY_DELAY_MS * (attempt + 1));
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
  return /Failed to fetch dynamically imported module|Importing a module script failed|Loading chunk \d+ failed|net::ERR_|NetworkError/i.test(
    message,
  );
}

function wait(delayMs: number): Promise<void> {
  return new Promise((resolve) => {
    globalThis.setTimeout(resolve, delayMs);
  });
}
