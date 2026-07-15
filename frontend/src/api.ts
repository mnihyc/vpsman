import type { JsonValue } from "./types";

export class ApiUnauthorizedError extends Error {
  constructor() {
    super("Operator login required");
    this.name = "ApiUnauthorizedError";
  }
}

export class ApiResponseError extends Error {
  status: number;
  code: string;
  detail: string | null;

  constructor(status: number, code: string, detail: string | null = null) {
    super(`${humanizeApiCode(code)}${detail ? `: ${detail}` : ""} (${status})`);
    this.name = "ApiResponseError";
    this.status = status;
    this.code = code;
    this.detail = detail;
  }
}

export function buildAuthHeaders(apiToken: string): HeadersInit | undefined {
  return apiToken ? { Authorization: `Bearer ${apiToken}` } : undefined;
}

export function buildJsonHeaders(apiToken: string): HeadersInit {
  return apiToken
    ? { Authorization: `Bearer ${apiToken}`, "Content-Type": "application/json" }
    : { "Content-Type": "application/json" };
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
  150,
  500,
  1_000,
  2_000,
  4_000,
  8_000,
  13_000,
];

async function fetchGetWithTransientRetry(
  path: string,
  apiToken: string,
): Promise<Response> {
  for (let attempt = 0; ; attempt += 1) {
    try {
      return await fetch(path, { headers: buildAuthHeaders(apiToken) });
    } catch (error) {
      const delay = GET_RETRY_DELAYS_MS[attempt];
      if (delay === undefined || !isTransientFetchFailure(error)) {
        throw error;
      }
      await wait(delay);
    }
  }
}

async function fetchPreviewPostWithTransientRetry(
  path: string,
  apiToken: string,
  body: unknown,
): Promise<Response> {
  const serializedBody = JSON.stringify(body);
  for (let attempt = 0; ; attempt += 1) {
    try {
      return await fetch(path, {
        method: "POST",
        headers: buildJsonHeaders(apiToken),
        body: serializedBody,
      });
    } catch (error) {
      const delay = PREVIEW_POST_RETRY_DELAYS_MS[attempt];
      if (delay === undefined || !isTransientFetchFailure(error)) {
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

export async function apiPost<T = JsonValue>(path: string, apiToken: string, body: unknown): Promise<T> {
  const response = await fetch(path, {
    method: "POST",
    headers: buildJsonHeaders(apiToken),
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
  return (await response.json()) as T;
}

export async function apiPostPreview<T = JsonValue>(
  path: string,
  apiToken: string,
  body: unknown,
): Promise<T> {
  const response = await fetchPreviewPostWithTransientRetry(path, apiToken, body);
  if (response.status === 401) {
    throw new ApiUnauthorizedError();
  }
  if (!response.ok) {
    throw await apiErrorFromResponse(response);
  }
  if (response.status === 204) {
    return undefined as T;
  }
  return (await response.json()) as T;
}

export async function apiPut<T = JsonValue>(path: string, apiToken: string, body: unknown): Promise<T> {
  const response = await fetch(path, {
    method: "PUT",
    headers: buildJsonHeaders(apiToken),
    body: JSON.stringify(body),
  });
  if (response.status === 401) {
    throw new ApiUnauthorizedError();
  }
  if (!response.ok) {
    throw await apiErrorFromResponse(response);
  }
  return (await response.json()) as T;
}

export async function apiPostBinary<T = JsonValue>(
  path: string,
  apiToken: string,
  body: Blob,
  headers: HeadersInit,
): Promise<T> {
  const requestHeaders = new Headers(headers);
  if (apiToken) {
    requestHeaders.set("Authorization", `Bearer ${apiToken}`);
  }
  const response = await fetch(path, {
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
  return (await response.json()) as T;
}

export async function apiGet<T = JsonValue>(path: string, apiToken: string): Promise<T> {
  const response = await fetchGetWithTransientRetry(path, apiToken);
  if (response.status === 401) {
    throw new ApiUnauthorizedError();
  }
  if (!response.ok) {
    throw await apiErrorFromResponse(response);
  }
  return (await response.json()) as T;
}

export async function apiGetBlob(path: string, apiToken: string): Promise<Blob> {
  const response = await fetchGetWithTransientRetry(path, apiToken);
  if (response.status === 401) {
    throw new ApiUnauthorizedError();
  }
  if (!response.ok) {
    throw await apiErrorFromResponse(response);
  }
  return await response.blob();
}

export async function apiDelete<T = JsonValue>(path: string, apiToken: string, body?: unknown): Promise<T> {
  const response = await fetch(path, {
    method: "DELETE",
    headers: body === undefined ? buildAuthHeaders(apiToken) : buildJsonHeaders(apiToken),
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
  return (await response.json()) as T;
}

export function isApiUnauthorized(error: unknown): error is ApiUnauthorizedError {
  return error instanceof ApiUnauthorizedError;
}

async function apiErrorFromResponse(response: Response): Promise<ApiResponseError> {
  let code = `http_${response.status}`;
  let detail: string | null = null;
  try {
    const contentType = response.headers.get("content-type") ?? "";
    if (contentType.includes("application/json")) {
      const body = (await response.json()) as { error?: unknown; message?: unknown };
      if (typeof body.error === "string" && body.error.trim()) {
        code = body.error;
      }
      if (typeof body.message === "string" && body.message.trim()) {
        detail = body.message.trim();
      }
    } else {
      const text = (await response.text()).trim();
      if (text) {
        code = text.slice(0, 160);
      }
    }
  } catch {
    code = `http_${response.status}`;
  }
  return new ApiResponseError(response.status, code, detail);
}

function humanizeApiCode(code: string): string {
  if (!code.trim()) {
    return "Request failed";
  }
  return code
    .replace(/_/g, " ")
    .replace(/\bapi\b/i, "API")
    .replace(/\b[a-z]/g, (letter) => letter.toUpperCase());
}
