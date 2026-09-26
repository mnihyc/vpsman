import type { AuthResponse, OperatorView } from "./types";
import { ACCESS_TOKEN_STORAGE_KEY, REFRESH_TOKEN_STORAGE_KEY } from "./constants";

/** Login identity, never a bearer token. Renewal preserves this object. */
export type AuthSession = Readonly<{ epoch: string }>;
export type SessionCredentials = Readonly<{
  accessToken: string;
  refreshToken: string;
  revision: number;
  attempt: string | null;
}>;
type SessionRecord = {
  epoch: string;
  status: "active" | "ended";
  revision: number;
  accessToken: string;
  refreshToken: string;
  operator: OperatorView | null;
  attempt: { id: string; error: string | null } | null;
};

const STORAGE_KEY = "vpsman.authSession.v1";
const LOCK_NAME = "vpsman.authSession";
const DATABASE_NAME = "vpsman.authSession";
const STORE_NAME = "session";
const CURRENT_KEY = "current";
const handles = new Map<string, AuthSession>();
const listeners = new Set<(session: AuthSession | null) => void>();
const credentialListeners = new Set<() => void>();
const authorityListeners = new Set<() => void>();
const errorListeners = new Set<(error: string | null) => void>();
const renewals = new Map<string, Promise<void>>();

export class AuthSessionEndedError extends Error {
  constructor() {
    super("This login has ended or was replaced. Sign in again.");
  }
}

export class AuthRenewalUnavailableError extends Error {}

export function authSessionEnvironmentError(): string | null {
  return typeof navigator !== "undefined" && navigator.locks && typeof globalThis.crypto?.randomUUID === "function" &&
    typeof globalThis.indexedDB !== "undefined"
    ? null
    : "Open the console over HTTPS or localhost in a browser supporting Web Locks and IndexedDB. Authenticated browser sessions require secure cross-tab coordination.";
}

function storedHint(): Partial<SessionRecord> | null {
  if (typeof window === "undefined") return null;
  const raw = window.localStorage.getItem(STORAGE_KEY);
  if (!raw) return null;
  try {
    const value = JSON.parse(raw);
    return value && typeof value === "object" ? value : null;
  } catch {
    return null;
  }
}

function isSessionRecord(value: unknown): value is SessionRecord {
  if (!value || typeof value !== "object") return false;
  const record = value as Partial<SessionRecord>;
  return typeof record.epoch === "string" &&
    (record.status === "active" || record.status === "ended") &&
    Number.isInteger(record.revision) &&
    typeof record.accessToken === "string" && typeof record.refreshToken === "string";
}

let databasePromise: Promise<IDBDatabase> | null = null;

function storageFailure(error: unknown): AuthRenewalUnavailableError {
  if (error instanceof AuthRenewalUnavailableError) return error;
  const detail = error instanceof Error ? ` ${error.message}` : "";
  return new AuthRenewalUnavailableError(`Browser session storage is unavailable.${detail} Restore IndexedDB access and retry; no fallback credentials were used.`);
}

function database(): Promise<IDBDatabase> {
  if (!databasePromise) {
    databasePromise = new Promise<IDBDatabase>((resolve, reject) => {
      const request = indexedDB.open(DATABASE_NAME, 1);
      let failed = false;
      request.onupgradeneeded = () => {
        if (!request.result.objectStoreNames.contains(STORE_NAME)) request.result.createObjectStore(STORE_NAME);
      };
      request.onerror = () => { failed = true; reject(storageFailure(request.error)); };
      request.onblocked = () => {
        failed = true;
        reject(storageFailure(new Error("Another page is blocking the session database upgrade.")));
      };
      request.onsuccess = () => {
        const connection = request.result;
        if (failed) { connection.close(); return; }
        connection.onversionchange = () => { connection.close(); databasePromise = null; };
        resolve(connection);
      };
    }).catch(error => { databasePromise = null; throw storageFailure(error); });
  }
  return databasePromise;
}

async function readCanonicalRecord(): Promise<SessionRecord | null> {
  const connection = await database();
  return new Promise((resolve, reject) => {
    const transaction = connection.transaction(STORE_NAME, "readonly");
    const request = transaction.objectStore(STORE_NAME).get(CURRENT_KEY);
    transaction.onabort = () => reject(storageFailure(transaction.error));
    transaction.oncomplete = () => {
      const value: unknown = request.result;
      if (value === undefined) resolve(null);
      else if (isSessionRecord(value)) resolve(value);
      else reject(storageFailure(new Error("The canonical session record is invalid.")));
    };
  });
}

async function writeCanonicalRecord(record: SessionRecord): Promise<void> {
  const connection = await database();
  await new Promise<void>((resolve, reject) => {
    const transaction = connection.transaction(STORE_NAME, "readwrite");
    transaction.objectStore(STORE_NAME).put(record, CURRENT_KEY);
    transaction.oncomplete = () => resolve();
    transaction.onabort = () => reject(storageFailure(transaction.error));
  });
}

function handle(record: SessionRecord | null): AuthSession | null {
  if (!record || record.status !== "active") return null;
  let session = handles.get(record.epoch);
  if (!session) {
    session = Object.freeze({ epoch: record.epoch });
    handles.set(record.epoch, session);
  }
  return session;
}

function authority(record: SessionRecord | null): string {
  const operator = record?.operator;
  return operator ? JSON.stringify([
    operator.id, operator.status, operator.role, [...operator.scopes].sort(),
  ]) : "";
}

let observed: SessionRecord | null = null;
let initialized = false;

function observeRecord(next: SessionRecord | null): void {
  const previous = observed;
  observed = next;
  initialized = true;
  if (handle(previous) !== handle(next)) {
    for (const listener of listeners) listener(handle(next));
  }
  if (previous?.epoch !== next?.epoch || previous?.status !== next?.status ||
      previous?.revision !== next?.revision) {
    for (const listener of credentialListeners) listener();
  }
  if (authority(previous) !== authority(next)) {
    for (const listener of authorityListeners) listener();
  }
  if (previous?.attempt?.error !== next?.attempt?.error) {
    for (const listener of errorListeners) listener(next?.attempt?.error ?? null);
  }
}

function synchronize(): void {
  void withStateLock(() => undefined).catch(error => {
    const message = error instanceof Error ? error.message : "Browser session storage is unavailable.";
    for (const listener of errorListeners) listener(message);
  });
}

if (typeof window !== "undefined") {
  window.addEventListener("storage", (event) => {
    if (event.storageArea === window.localStorage && (event.key === STORAGE_KEY || event.key === null)) {
      synchronize();
    }
  });
  document.addEventListener("visibilitychange", () => {
    if (!document.hidden) synchronize();
  });
}

function publishHint(record: SessionRecord): void {
  // Notification only. Renderer-local storage caches are not transactional and
  // must never decide which credential pair owns a cross-tab renewal.
  window.localStorage.setItem(STORAGE_KEY, JSON.stringify({
    epoch: record.epoch, status: record.status, revision: record.revision,
    attempt: record.attempt?.id ?? null,
  }));
}

async function publish(record: SessionRecord): Promise<void> {
  await writeCanonicalRecord(record);
  observeRecord(record);
  publishHint(record);
}

async function withAuthLock<T>(work: () => Promise<T>): Promise<T> {
  const unavailable = authSessionEnvironmentError();
  if (unavailable) throw new AuthRenewalUnavailableError(unavailable);
  return navigator.locks.request(LOCK_NAME, work);
}

// State commits never wait on HTTP. Logout can invalidate a login while its
// renewal is in flight, without a check/write race or a timed storage lease.
async function withStateLock<T>(work: () => T | Promise<T>): Promise<T> {
  const unavailable = authSessionEnvironmentError();
  if (unavailable) throw new AuthRenewalUnavailableError(unavailable);
  return navigator.locks.request(`${LOCK_NAME}.state`, async () => {
    // IndexedDB completion, not a localStorage notification, is the canonical
    // read barrier between renderer processes. Cache updates stay serialized.
    observeRecord(await readCanonicalRecord());
    return await work();
  });
}

function current(session: AuthSession): SessionRecord {
  const record = observed;
  if (!record || record.status !== "active" || record.epoch !== session.epoch) {
    throw new AuthSessionEndedError();
  }
  return record;
}

export function getCurrentAuthSession(): AuthSession | null {
  if (authSessionEnvironmentError()) return null;
  return handle(observed);
}

export function hasStoredAuthSession(): boolean {
  if (authSessionEnvironmentError()) return false;
  if (initialized) return getCurrentAuthSession() !== null;
  const hint = storedHint();
  if (hint?.status === "active" || hint?.status === "ended") return hint.status === "active";
  return Boolean(window.localStorage.getItem(ACCESS_TOKEN_STORAGE_KEY) ||
    window.localStorage.getItem(REFRESH_TOKEN_STORAGE_KEY));
}

export function captureAuthBoundary(): string {
  const record = observed;
  return record ? `${record.epoch}:${record.status}` : "absent";
}

export function readSessionCredentials(session: AuthSession): SessionCredentials | null {
  const record = observed;
  return record?.epoch === session.epoch && record.status === "active"
    ? { accessToken: record.accessToken, refreshToken: record.refreshToken, revision: record.revision, attempt: record.attempt?.id ?? null }
    : null;
}

/** Authoritative credential dispatch/fence read; never relies on a renderer cache. */
export async function readCurrentSessionCredentials(session: AuthSession): Promise<SessionCredentials> {
  return withStateLock(() => {
    const record = current(session);
    return {accessToken: record.accessToken, refreshToken: record.refreshToken,
      revision: record.revision, attempt: record.attempt?.id ?? null};
  });
}

export function getSessionOperator(session: AuthSession): OperatorView | null {
  const record = observed;
  return record?.epoch === session.epoch && record.status === "active" ? record.operator : null;
}

/** Bind the identity loaded for a legacy pair without replacing fresher authority. */
export async function rememberSessionOperator(session: AuthSession, operator: OperatorView): Promise<void> {
  await withStateLock(async () => {
    const record = current(session);
    if (!record.operator) await publish({ ...record, operator });
  });
}

export function subscribeAuthSession(listener: (session: AuthSession | null) => void): () => void {
  listeners.add(listener);
  return () => { listeners.delete(listener); };
}

export function subscribeAuthRefreshError(listener: (error: string | null) => void): () => void {
  errorListeners.add(listener);
  listener(observed?.attempt?.error ?? null);
  return () => { errorListeners.delete(listener); };
}

export function subscribeSessionCredentials(_session: AuthSession, listener: () => void): () => void {
  const notify = () => { listener(); };
  credentialListeners.add(notify);
  return () => { credentialListeners.delete(notify); };
}

export function subscribeSessionAuthority(session: AuthSession, listener: () => void): () => void {
  const notify = () => { if (getCurrentAuthSession() === session) listener(); };
  authorityListeners.add(notify);
  return () => { authorityListeners.delete(notify); };
}

export async function initializeAuthSession(): Promise<AuthSession | null> {
  return withStateLock(async () => {
    if (!observed) {
      const legacy = storedHint();
      if (isSessionRecord(legacy)) {
        await publish(legacy);
      } else {
        const accessToken = window.localStorage.getItem(ACCESS_TOKEN_STORAGE_KEY) ?? "";
        const refreshToken = window.localStorage.getItem(REFRESH_TOKEN_STORAGE_KEY) ?? "";
        if (accessToken || refreshToken) {
          await publish({ epoch: crypto.randomUUID(), status: "active", revision: 0,
            accessToken, refreshToken, operator: null, attempt: null });
        }
      }
    }
    // A canonical active or ended record always wins over legacy credentials.
    if (observed) publishHint(observed);
    window.localStorage.removeItem(ACCESS_TOKEN_STORAGE_KEY);
    window.localStorage.removeItem(REFRESH_TOKEN_STORAGE_KEY);
    return getCurrentAuthSession();
  });
}

async function revoke(accessToken: string): Promise<void> {
  if (!accessToken) return;
  const response = await fetch("/api/v1/auth/logout", {
    method: "POST", headers: { Authorization: `Bearer ${accessToken}`, "Content-Type": "application/json" }, body: "{}",
  });
  // A rejected bearer is already unusable; other errors leave revocation unknown.
  if (!response.ok && response.status !== 401) throw new Error("Server session revocation could not be confirmed.");
}

async function end(record: SessionRecord): Promise<void> {
  await publish({ ...record, status: "ended", accessToken: "", refreshToken: "", operator: null, attempt: null });
}

export async function installAuthSession(auth: AuthResponse, boundary = captureAuthBoundary()): Promise<AuthSession> {
  const session = await withStateLock(async () => {
    if (captureAuthBoundary() !== boundary) {
      return null;
    }
    const record: SessionRecord = { epoch: crypto.randomUUID(), status: "active", revision: 0,
      accessToken: auth.access_token, refreshToken: auth.refresh_token, operator: auth.operator, attempt: null };
    await publish(record);
    return handle(record)!;
  });
  if (!session) {
    await revoke(auth.access_token);
    throw new AuthSessionEndedError();
  }
  return session;
}

export async function logoutAuthSession(session: AuthSession): Promise<void> {
  const record = await withStateLock(async () => {
    const record = observed;
    if (!record || record.epoch !== session.epoch) throw new AuthSessionEndedError();
    // Another caller may already have ended this same login. Join cleanup
    // without reporting a false revocation failure or erasing its warning.
    if (record.status === "active") await end(record);
    return record;
  });
  await withAuthLock(async () => {
    // A renewal that finished after local logout owns revoking its replacement.
    // Do not mistake rejection of the spent old bearer for successful cleanup.
    const settled = await withStateLock(() => observed);
    if (settled?.epoch === session.epoch && settled.status === "ended" && settled.attempt?.error) {
      throw new Error(settled.attempt.error);
    }
    await revoke(record.accessToken);
  });
}

export function renewAuthSession(session: AuthSession, challengedRevision: number, challengedAttempt?: string | null, retryFailedAttempt = false): Promise<void> {
  const running = renewals.get(session.epoch);
  if (running) return running;
  const record = observed?.epoch === session.epoch ? observed : null;
  const observedAttempt = challengedAttempt === undefined ? (record?.attempt?.id ?? null) : challengedAttempt;
  const request = withAuthLock(async () => {
    const latest = await withStateLock(() => current(session));
    if (latest.revision !== challengedRevision) return;
    // Background polling cannot turn one transient failure into an endless
    // refresh loop. The existing Retry action owns another failed attempt.
    if (latest.attempt?.error && (!retryFailedAttempt || latest.attempt.id !== observedAttempt)) {
      throw new AuthRenewalUnavailableError(latest.attempt.error);
    }
    if (!latest.refreshToken) {
      await withStateLock(async () => { if (getCurrentAuthSession() === session) await end(current(session)); });
      throw new AuthSessionEndedError();
    }
    try {
      const response = await fetch("/api/v1/auth/refresh", {
        method: "POST", headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ refresh_token: latest.refreshToken }),
      });
      if (response.status === 401) {
        await withStateLock(async () => {
          if (getCurrentAuthSession() === session && current(session).revision === challengedRevision) await end(current(session));
        });
        throw new AuthSessionEndedError();
      }
      if (!response.ok) throw new Error(`Session renewal returned HTTP ${response.status}.`);
      const auth = await response.json() as AuthResponse;
      if (!auth.access_token || !auth.refresh_token || !auth.operator) throw new Error("Session renewal returned an incomplete response.");
      const accepted = await withStateLock(async () => {
        if (getCurrentAuthSession() !== session) return false;
        const active = current(session);
        if (active.revision !== challengedRevision) return false;
        if (active.operator && active.operator.id !== auth.operator.id) {
          await end(active);
          return false;
        }
        await publish({ ...active, revision: active.revision + 1,
          accessToken: auth.access_token, refreshToken: auth.refresh_token,
          operator: auth.operator, attempt: { id: crypto.randomUUID(), error: null } });
        return true;
      });
      if (!accepted) {
        // A committed replacement received after logout/account switch is
        // revoked, never published back into the authenticated workspace.
        try {
          await revoke(auth.access_token);
        } catch (error) {
          const message = error instanceof Error ? error.message : "Server session revocation could not be confirmed.";
          await withStateLock(async () => {
            const ended = observed;
            if (ended?.epoch === session.epoch && ended.status === "ended") {
              await publish({ ...ended, attempt: { id: crypto.randomUUID(), error: message } });
            }
          });
          throw error;
        }
        throw new AuthSessionEndedError();
      }
    } catch (error) {
      if (error instanceof AuthSessionEndedError) throw error;
      const message = error instanceof Error ? error.message : "Session renewal is unavailable.";
      await withStateLock(async () => {
        if (getCurrentAuthSession() === session && current(session).revision === challengedRevision) {
          await publish({ ...current(session), attempt: { id: crypto.randomUUID(), error: message } });
        }
      });
      throw new AuthRenewalUnavailableError(message);
    }
  });
  renewals.set(session.epoch, request);
  void request.finally(() => {
    if (renewals.get(session.epoch) === request) renewals.delete(session.epoch);
  }).catch(() => undefined);
  return request;
}
