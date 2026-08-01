import {
  useCallback,
  useEffect,
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
  const currentEntryId = enabled ? ensureHistoryEntryId() : null;
  const activeEntryRef = useRef({ entryId: currentEntryId, slot });
  const initialRef = useRef(initial);
  initialRef.current = initial;
  const [value, setValue] = useState<T>(() => {
    if (currentEntryId) {
      const stored = entryState.get(currentEntryId);
      if (stored?.has(slot)) {
        return stored.get(slot) as T;
      }
      const resolved = initialValue(initial);
      const snapshot = stored ?? new Map<string, unknown>();
      snapshot.set(slot, resolved);
      entryState.set(currentEntryId, snapshot);
      return resolved;
    }
    return initialValue(initial);
  });
  const valueRef = useRef(value);
  valueRef.current = value;

  useEffect(() => {
    const active = activeEntryRef.current;
    if (active.entryId === currentEntryId && active.slot === slot) {
      return;
    }
    activeEntryRef.current = { entryId: currentEntryId, slot };
    const stored = currentEntryId ? entryState.get(currentEntryId) : null;
    const resolved = stored?.has(slot)
      ? (stored.get(slot) as T)
      : initialValue(initialRef.current);
    if (currentEntryId && !stored?.has(slot)) {
      const snapshot = stored ?? new Map<string, unknown>();
      snapshot.set(slot, resolved);
      entryState.set(currentEntryId, snapshot);
    }
    valueRef.current = resolved;
    setValue(resolved);
  }, [currentEntryId, slot]);

  const setHistoryValue = useCallback<Dispatch<SetStateAction<T>>>(
    (next) => {
      const resolved =
        typeof next === "function"
          ? (next as (current: T) => T)(valueRef.current)
          : next;
      valueRef.current = resolved;
      const entryId = activeEntryRef.current.entryId;
      if (entryId) {
        const stored = entryState.get(entryId) ?? new Map<string, unknown>();
        stored.set(activeEntryRef.current.slot, resolved);
        entryState.set(entryId, stored);
      }
      setValue(resolved);
    },
    [slot],
  );

  return [value, setHistoryValue];
}
