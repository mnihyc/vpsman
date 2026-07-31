import {
  useCallback,
  useRef,
  useState,
  type Dispatch,
  type SetStateAction,
} from "react";

const HISTORY_METADATA_KEY = "__vpsman_history";
const HISTORY_METADATA_VERSION = 1;

type HistoryMetadata = {
  entryId: string;
  version: typeof HISTORY_METADATA_VERSION;
};

// Payloads stay process-local by design: Back/Forward can restore them, while a
// reload clears them. Browser history receives only the opaque entry identity.
const entryState = new Map<string, Map<string, unknown>>();

function historyRecord(): Record<string, unknown> {
  const current = window.history.state;
  if (!current || typeof current !== "object" || Array.isArray(current)) {
    return {};
  }
  return current as Record<string, unknown>;
}

function readEntryId(): string | null {
  const metadata = historyRecord()[HISTORY_METADATA_KEY];
  if (
    !metadata ||
    typeof metadata !== "object" ||
    Array.isArray(metadata)
  ) {
    return null;
  }
  const candidate = metadata as Partial<HistoryMetadata>;
  return candidate.version === HISTORY_METADATA_VERSION &&
    typeof candidate.entryId === "string" &&
    candidate.entryId
    ? candidate.entryId
    : null;
}

function historyStateWithEntryId(entryId: string): Record<string, unknown> {
  return {
    ...historyRecord(),
    [HISTORY_METADATA_KEY]: {
      entryId,
      version: HISTORY_METADATA_VERSION,
    } satisfies HistoryMetadata,
  };
}

export function ensureHistoryEntryId(): string {
  const existing = readEntryId();
  if (existing) {
    return existing;
  }
  const entryId = crypto.randomUUID();
  window.history.replaceState(
    historyStateWithEntryId(entryId),
    "",
    window.location.href,
  );
  return entryId;
}

export function pushHistoryEntry(url: string): void {
  ensureHistoryEntryId();
  const entryId = crypto.randomUUID();
  window.history.pushState(historyStateWithEntryId(entryId), "", url);
}

export function replaceHistoryEntry(url: string): void {
  const entryId = ensureHistoryEntryId();
  window.history.replaceState(historyStateWithEntryId(entryId), "", url);
}

function initialValue<T>(value: T | (() => T)): T {
  return typeof value === "function" ? (value as () => T)() : value;
}

export function useHistoryEntryState<T>(
  slot: string,
  initial: T | (() => T),
  enabled = true,
): [T, Dispatch<SetStateAction<T>>] {
  const entryIdRef = useRef<string | null>(null);
  if (enabled && !entryIdRef.current) {
    entryIdRef.current = ensureHistoryEntryId();
  }
  const [value, setValue] = useState<T>(() => {
    const entryId = entryIdRef.current;
    if (entryId) {
      const stored = entryState.get(entryId);
      if (stored?.has(slot)) {
        return stored.get(slot) as T;
      }
      const resolved = initialValue(initial);
      const snapshot = stored ?? new Map<string, unknown>();
      snapshot.set(slot, resolved);
      entryState.set(entryId, snapshot);
      return resolved;
    }
    return initialValue(initial);
  });
  const valueRef = useRef(value);
  valueRef.current = value;

  const setHistoryValue = useCallback<Dispatch<SetStateAction<T>>>(
    (next) => {
      const resolved =
        typeof next === "function"
          ? (next as (current: T) => T)(valueRef.current)
          : next;
      valueRef.current = resolved;
      const entryId = entryIdRef.current;
      if (entryId) {
        const stored = entryState.get(entryId) ?? new Map<string, unknown>();
        stored.set(slot, resolved);
        entryState.set(entryId, stored);
      }
      setValue(resolved);
    },
    [slot],
  );

  return [value, setHistoryValue];
}
