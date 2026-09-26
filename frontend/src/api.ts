import type { JsonValue } from "./types";
import { AuthSessionEndedError, getCurrentAuthSession, readCurrentSessionCredentials, renewAuthSession, type AuthSession } from "./authSession";

const responseSessions = new WeakMap<Response, AuthSession>();

export function assertResponseSessionCurrent(response: Response): void {
  const session = responseSessions.get(response);
  if (session && getCurrentAuthSession() !== session) throw new ApiUnauthorizedError();
}

/** Fence completed bodies against committed cross-tab logout/account changes. */
export async function verifyResponseSessionCurrent(response: Response): Promise<void> {
  const session = responseSessions.get(response);
  if (!session) return;
  try {
    await readCurrentSessionCredentials(session);
  } catch (error) {
    if (error instanceof AuthSessionEndedError) throw new ApiUnauthorizedError();
    throw error;
  }
}

export class ApiUnauthorizedError extends Error {
  constructor() {
    super(
      "The operator session is absent or expired. Sign in again before retrying this action.",
    );
    this.name = "ApiUnauthorizedError";
  }
}

export class ApiTransportError extends Error {
  constructor(browserDetail: string | null = null, readonly transient = false) {
    const detail = browserDetail ? ` Browser reported: ${browserDetail}.` : "";
    super(
      `The control plane did not return a readable response.${detail} Check API availability, TLS, reverse-proxy routing, and same-origin/CORS configuration before retrying. No success is assumed.`,
    );
    this.name = "ApiTransportError";
  }
}

export class ApiResponseError extends Error {
  status: number;
  code: string;
  detail: string | null;
  recovery: string;

  constructor(
    status: number,
    code: string,
    detail: string | null = null,
    recovery: string | null = null,
  ) {
    const summary = `${humanizeApiCode(code)}${detail ? `: ${detail}` : ""} (${status})`;
    const operatorRecovery = recovery?.trim() || apiErrorGuidance(status, code);
    super(`${summary}. ${operatorRecovery}`);
    this.name = "ApiResponseError";
    this.status = status;
    this.code = code;
    this.detail = detail;
    this.recovery = operatorRecovery;
  }
}

/**
 * Owns one browser-local aggregate read at a time. Producers replace the
 * pending work with their latest desired read; the active consumer drains that
 * trailing work immediately. This is deliberately not a timer, retry policy,
 * cache, or cross-browser concurrency limit.
 */
export class LatestReadConsumer<T = void> {
  private running = false;
  private pending: {
    run: () => Promise<T>;
    waiters: Array<{
      reject: (reason: unknown) => void;
      resolve: (value: T) => void;
    }>;
  } | null = null;

  enqueue(work: () => Promise<T>): Promise<T> {
    const result = new Promise<T>((resolve, reject) => {
      if (this.pending === null) {
        this.pending = { run: work, waiters: [{ reject, resolve }] };
      } else {
        this.pending.run = work;
        this.pending.waiters.push({ reject, resolve });
      }
    });
    if (!this.running) {
      this.running = true;
      void this.drain();
    }
    return result;
  }

  discardPending(result?: T): void {
    for (const waiter of this.pending?.waiters ?? []) {
      waiter.resolve(result as T);
    }
    this.pending = null;
  }

  private async drain(): Promise<void> {
    try {
      for (;;) {
        const work = this.pending;
        if (work === null) {
          // Close the handoff against already-queued producer microtasks without
          // adding a polling interval or delaying a real request.
          await Promise.resolve();
          if (this.pending === null) return;
          continue;
        }
        this.pending = null;
        try {
          const result = await work.run();
          for (const waiter of work.waiters) waiter.resolve(result);
        } catch (error) {
          for (const waiter of work.waiters) waiter.reject(error);
        }
      }
    } finally {
      this.running = false;
      if (this.pending !== null) {
        // A producer may run in the promise-finalization handoff. Give that
        // already-recorded latest work a new owner without synthesizing work.
        this.running = true;
        void this.drain();
      }
    }
  }
}

function apiErrorGuidance(status: number, code: string): string {
  if (code === "hostname_resolution_timeout") {
    return "The control plane DNS lookup exceeded its five-second limit; verify resolver reachability and retry.";
  }
  if (code === "hostname_resolution_failed") {
    return "The control plane resolver could not complete the lookup; verify the hostname and server DNS configuration before retrying.";
  }
  if (code === "hostname_resolution_no_addresses") {
    return "The hostname returned no usable unicast IPv4 or IPv6 address; correct DNS or enter a literal target IP.";
  }
  if (code === "hostname_invalid") {
    return "Enter a valid DNS hostname, or use the literal target-IP field instead.";
  }
  if (code.includes("snapshot_stale") || code.includes("confirmation_stale")) {
    return "The reviewed state changed; refresh it and review the action again.";
  }
  if (code.includes("confirmation") || code.includes("preview_hash")) {
    return "Review the current action snapshot before submitting it again.";
  }
  if (code.includes("capability") || code.includes("unsupported")) {
    return "The selected VPS does not currently advertise the required capability; inspect its agent status before retrying.";
  }
  switch (status) {
    case 400:
      return "Review the submitted values and correct the invalid field before retrying.";
    case 401:
      return "Sign in again before retrying this action.";
    case 403:
      return "The current operator scope or privilege unlock does not permit this action.";
    case 404:
      return "The target no longer exists or is outside the current operator scope; refresh the page.";
    case 409:
      return "The requested change conflicts with current state; refresh and review before retrying.";
    case 413:
      return "Reduce the request or artifact size and retry.";
    case 429:
      return "Wait for the active limit or cooldown to clear before retrying.";
    default:
      if (status >= 500) {
        return "The server did not complete the action and no success is assumed; inspect API logs and retry.";
      }
      return "The action did not complete; refresh current state before retrying.";
  }
}

export type ListQueryParams = {
  dir?: "asc" | "desc";
  limit?: number;
  offset?: number;
  q?: string;
  sort?: string;
};

export function buildListPath(path: string, query: ListQueryParams): string {
  const params = new URLSearchParams();
  if (query.limit !== undefined) params.set("limit", String(query.limit));
  if (query.offset !== undefined) params.set("offset", String(query.offset));
  if (query.sort) params.set("sort", query.sort);
  if (query.dir) params.set("dir", query.dir);
  if (query.q?.trim()) params.set("q", query.q.trim());
  const suffix = params.toString();
  return suffix ? `${path}?${suffix}` : path;
}

const GET_RETRY_DELAYS_MS = [150, 500, 1_000];
const PREVIEW_POST_RETRY_DELAYS_MS = [
  150, 500, 1_000, 2_000, 4_000, 8_000, 13_000,
];

async function fetchGetWithTransientRetry(
  path: string,
  apiToken: AuthSession | null,
): Promise<Response> {
  let transportAttempt = 0;
  for (;;) {
    try {
      return await authenticatedApiFetch(path, apiToken);
    } catch (error) {
      const delay = GET_RETRY_DELAYS_MS[transportAttempt];
      if (delay === undefined || !(error instanceof ApiTransportError && error.transient)) {
        throw error;
      }
      transportAttempt += 1;
      await wait(delay);
    }
  }
}

async function fetchPreviewPostWithTransientRetry(
  path: string,
  apiToken: AuthSession | null,
  body: unknown,
): Promise<Response> {
  const serializedBody = JSON.stringify(body);
  for (let attempt = 0; ; attempt += 1) {
    try {
      return await authenticatedApiFetch(path, apiToken, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: serializedBody,
      }, true);
    } catch (error) {
      const delay = PREVIEW_POST_RETRY_DELAYS_MS[attempt];
      if (delay === undefined || !(error instanceof ApiTransportError && error.transient)) {
        throw error;
      }
      await wait(delay);
    }
  }
}

function isTransientFetchFailure(error: unknown): boolean {
  if (!(error instanceof TypeError)) {
    return false;
  }
  return /fetch|network|load|failed/i.test(error.message);
}

function wait(delayMs: number): Promise<void> {
  return new Promise((resolve) => globalThis.setTimeout(resolve, delayMs));
}

export async function apiFetch(
  input: RequestInfo | URL,
  init?: RequestInit,
): Promise<Response> {
  try {
    return await fetch(input, init);
  } catch (error) {
    throw apiTransportError(error);
  }
}

/** Credential recovery belongs to transport, not the component lifetime. */
export async function authenticatedApiFetch(
  input: RequestInfo | URL,
  session: AuthSession | null,
  init: RequestInit = {},
  safeAuthRetry = ["GET", "HEAD"].includes(init.method ?? "GET"),
): Promise<Response> {
  if (!session) return apiFetch(input, init);
  try {
    let credentials = await readCurrentSessionCredentials(session);
    if (!credentials.accessToken) {
      await renewAuthSession(session, credentials.revision, credentials.attempt);
      credentials = await readCurrentSessionCredentials(session);
    }
    const send = async (accessToken: string) => {
      if (getCurrentAuthSession() !== session) throw new AuthSessionEndedError();
      const headers = new Headers(init.headers);
      headers.set("Authorization", `Bearer ${accessToken}`);
      let response: Response;
      try {
        response = await apiFetch(input, { ...init, headers });
      } finally {
        // Includes network errors and bodyless responses, not only JSON reads.
        await readCurrentSessionCredentials(session);
      }
      responseSessions.set(response, session);
      return response;
    };
    let response = await send(credentials.accessToken);
    if (response.status !== 401) return response;
    await renewAuthSession(session, credentials.revision, credentials.attempt);
    if (!safeAuthRetry) {
      throw new Error("The request was rejected while credentials changed. Your session has recovered; review and retry the action. It was not automatically resubmitted.");
    }
    const renewed = await readCurrentSessionCredentials(session);
    response = await send(renewed.accessToken);
    if (response.status === 401) {
      // One recovery cannot become an unbounded renewal loop. Only refresh's
      // authoritative rejection ends the login, not an individual API error.
      throw await apiErrorFromResponse(response);
    }
    return response;
  } catch (error) {
    if (error instanceof AuthSessionEndedError) throw new ApiUnauthorizedError();
    throw error;
  }
}

function apiTransportError(error: unknown): ApiTransportError {
  if (error instanceof ApiTransportError) {
    return error;
  }
  const browserDetail =
    error instanceof Error && error.message.trim()
      ? error.message.trim().replace(/\s+/g, " ").slice(0, 160)
      : null;
  return new ApiTransportError(browserDetail, isTransientFetchFailure(error));
}

export async function apiPost<T = JsonValue>(
  path: string,
  apiToken: AuthSession | null,
  body: unknown,
  signal?: AbortSignal,
): Promise<T> {
  const response = await authenticatedApiFetch(path, apiToken, {
    signal,
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  });
  if (response.status === 401) {
    throw new ApiUnauthorizedError();
  }
  if (!response.ok) {
    throw await apiErrorFromResponse(response);
  }
  if (response.status === 204) {
    return undefined as T;
  }
  return await apiJsonFromResponse<T>(response, `POST ${path}`);
}

export async function apiPostPreview<T = JsonValue>(
  path: string,
  apiToken: AuthSession | null,
  body: unknown,
): Promise<T> {
  const response = await fetchPreviewPostWithTransientRetry(
    path,
    apiToken,
    body,
  );
  if (response.status === 401) {
    throw new ApiUnauthorizedError();
  }
  if (!response.ok) {
    throw await apiErrorFromResponse(response);
  }
  if (response.status === 204) {
    return undefined as T;
  }
  return await apiJsonFromResponse<T>(response, `POST ${path}`);
}

export async function apiPut<T = JsonValue>(
  path: string,
  apiToken: AuthSession | null,
  body: unknown,
): Promise<T> {
  const response = await authenticatedApiFetch(path, apiToken, {
    method: "PUT",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  });
  if (response.status === 401) {
    throw new ApiUnauthorizedError();
  }
  if (!response.ok) {
    throw await apiErrorFromResponse(response);
  }
  return await apiJsonFromResponse<T>(response, `PUT ${path}`);
}

export async function apiPatch<T = JsonValue>(
  path: string,
  apiToken: AuthSession | null,
  body: unknown,
): Promise<T> {
  const response = await authenticatedApiFetch(path, apiToken, {
    method: "PATCH",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body),
  });
  if (response.status === 401) throw new ApiUnauthorizedError();
  if (!response.ok) throw await apiErrorFromResponse(response);
  return await apiJsonFromResponse<T>(response, `PATCH ${path}`);
}

export async function apiPostBinary<T = JsonValue>(
  path: string,
  apiToken: AuthSession | null,
  body: Blob,
  headers: HeadersInit,
): Promise<T> {
  const requestHeaders = new Headers(headers);
  const response = await authenticatedApiFetch(path, apiToken, {
    method: "POST",
    headers: requestHeaders,
    body,
  });
  if (response.status === 401) {
    throw new ApiUnauthorizedError();
  }
  if (!response.ok) {
    throw await apiErrorFromResponse(response);
  }
  return await apiJsonFromResponse<T>(response, `POST ${path}`);
}

export async function apiGet<T = JsonValue>(
  path: string,
  apiToken: AuthSession | null,
): Promise<T> {
  const response = await fetchGetWithTransientRetry(path, apiToken);
  if (response.status === 401) {
    throw new ApiUnauthorizedError();
  }
  if (!response.ok) {
    throw await apiErrorFromResponse(response);
  }
  return await apiJsonFromResponse<T>(response, `GET ${path}`);
}

export async function apiGetBlob(
  path: string,
  apiToken: AuthSession | null,
): Promise<Blob> {
  const response = await fetchGetWithTransientRetry(path, apiToken);
  if (response.status === 401) {
    throw new ApiUnauthorizedError();
  }
  if (!response.ok) {
    throw await apiErrorFromResponse(response);
  }
  try {
    return await response.blob();
  } finally {
    await verifyResponseSessionCurrent(response);
  }
}

export async function apiDelete<T = JsonValue>(
  path: string,
  apiToken: AuthSession | null,
  body?: unknown,
): Promise<T> {
  const response = await authenticatedApiFetch(path, apiToken, {
    method: "DELETE",
    headers:
      body === undefined ? undefined : { "Content-Type": "application/json" },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  if (response.status === 401) {
    throw new ApiUnauthorizedError();
  }
  if (!response.ok) {
    throw await apiErrorFromResponse(response);
  }
  if (response.status === 204) {
    return undefined as T;
  }
  return await apiJsonFromResponse<T>(response, `DELETE ${path}`);
}

export function isApiUnauthorized(
  error: unknown,
): error is ApiUnauthorizedError {
  return error instanceof ApiUnauthorizedError;
}

export async function apiErrorFromResponse(
  response: Response,
): Promise<ApiResponseError> {
  let code = `http_${response.status}`;
  let detail: string | null = null;
  let recovery: string | null = null;
  try {
    const contentType = response.headers.get("content-type") ?? "";
    if (contentType.includes("application/json")) {
      const body = (await response.json()) as {
        error?: unknown;
        message?: unknown;
        recovery?: unknown;
      };
      if (typeof body.error === "string" && body.error.trim()) {
        code = body.error;
      }
      if (typeof body.message === "string" && body.message.trim()) {
        detail = body.message.trim();
      }
      if (typeof body.recovery === "string" && body.recovery.trim()) {
        recovery = body.recovery.trim();
      }
    } else {
      const text = (await response.text()).trim();
      if (text) {
        detail = nonJsonErrorDetail(response, text);
      }
    }
  } catch (error) {
    code = `http_${response.status}`;
    detail = `The server returned an unreadable error body${browserErrorDetail(error)}`;
  }
  await verifyResponseSessionCurrent(response);
  if (!detail && code === `http_${response.status}`) {
    detail =
      response.statusText.trim() ||
      "The server returned no explanatory error body";
  }
  return new ApiResponseError(response.status, code, detail, recovery);
}

function nonJsonErrorDetail(response: Response, text: string): string {
  const contentType = (
    response.headers.get("content-type") ?? ""
  ).toLowerCase();
  if (
    contentType.includes("text/html") ||
    /<(?:!doctype|html|head|body|title|center|h1)\b/i.test(text)
  ) {
    return "The reverse proxy returned an HTML error page instead of an API response";
  }
  return text
    .replace(/[\u0000-\u001f\u007f]+/g, " ")
    .replace(/\s+/g, " ")
    .slice(0, 240);
}

export async function apiJsonFromResponse<T>(
  response: Response,
  requestLabel: string,
): Promise<T> {
  let value: T;
  try {
    value = (await response.json()) as T;
  } catch (error) {
    await verifyResponseSessionCurrent(response);
    throw new Error(
      `The control plane reported HTTP ${response.status} for ${requestLabel}, but returned unreadable JSON${browserErrorDetail(error)}. Current state cannot be inferred from this response; refresh it and inspect reverse-proxy or API logs before repeating any mutation.`,
    );
  }
  await verifyResponseSessionCurrent(response);
  return value;
}

function browserErrorDetail(error: unknown): string {
  if (!(error instanceof Error) || !error.message.trim()) {
    return "";
  }
  return `; browser reported: ${error.message.trim().replace(/\s+/g, " ").slice(0, 160)}`;
}

function humanizeApiCode(code: string): string {
  if (!code.trim()) {
    return "Request failed";
  }
  return code
    .replace(/_/g, " ")
    .replace(/\bapi\b/i, "API")
    .replace(/\bhttp\b/i, "HTTP")
    .replace(/\b[a-z]/g, (letter) => letter.toUpperCase());
}
