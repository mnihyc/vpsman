import type { JsonValue } from "./types";

export class ApiUnauthorizedError extends Error {
  constructor() {
    super("Operator login required");
    this.name = "ApiUnauthorizedError";
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
    throw new Error(`API ${response.status}`);
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
    throw new Error(`API ${response.status}`);
  }
  return (await response.json()) as T;
}

export async function apiGet<T = JsonValue>(path: string, apiToken: string): Promise<T> {
  const response = await fetch(path, { headers: buildAuthHeaders(apiToken) });
  if (response.status === 401) {
    throw new ApiUnauthorizedError();
  }
  if (!response.ok) {
    throw new Error(`API ${response.status}`);
  }
  return (await response.json()) as T;
}

export async function apiGetBlob(path: string, apiToken: string): Promise<Blob> {
  const response = await fetch(path, { headers: buildAuthHeaders(apiToken) });
  if (response.status === 401) {
    throw new ApiUnauthorizedError();
  }
  if (!response.ok) {
    throw new Error(`API ${response.status}`);
  }
  return await response.blob();
}

export async function apiDelete<T = JsonValue>(path: string, apiToken: string): Promise<T> {
  const response = await fetch(path, { method: "DELETE", headers: buildAuthHeaders(apiToken) });
  if (response.status === 401) {
    throw new ApiUnauthorizedError();
  }
  if (!response.ok) {
    throw new Error(`API ${response.status}`);
  }
  return (await response.json()) as T;
}

export function isApiUnauthorized(error: unknown): error is ApiUnauthorizedError {
  return error instanceof ApiUnauthorizedError;
}
