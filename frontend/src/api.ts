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

  constructor(status: number, code: string) {
    super(`${humanizeApiCode(code)} (${status})`);
    this.name = "ApiResponseError";
    this.status = status;
    this.code = code;
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
  const response = await fetch(path, { headers: buildAuthHeaders(apiToken) });
  if (response.status === 401) {
    throw new ApiUnauthorizedError();
  }
  if (!response.ok) {
    throw await apiErrorFromResponse(response);
  }
  return (await response.json()) as T;
}

export async function apiGetBlob(path: string, apiToken: string): Promise<Blob> {
  const response = await fetch(path, { headers: buildAuthHeaders(apiToken) });
  if (response.status === 401) {
    throw new ApiUnauthorizedError();
  }
  if (!response.ok) {
    throw await apiErrorFromResponse(response);
  }
  return await response.blob();
}

export async function apiDelete<T = JsonValue>(path: string, apiToken: string): Promise<T> {
  const response = await fetch(path, { method: "DELETE", headers: buildAuthHeaders(apiToken) });
  if (response.status === 401) {
    throw new ApiUnauthorizedError();
  }
  if (!response.ok) {
    throw await apiErrorFromResponse(response);
  }
  return (await response.json()) as T;
}

export function isApiUnauthorized(error: unknown): error is ApiUnauthorizedError {
  return error instanceof ApiUnauthorizedError;
}

async function apiErrorFromResponse(response: Response): Promise<ApiResponseError> {
  let code = `http_${response.status}`;
  try {
    const contentType = response.headers.get("content-type") ?? "";
    if (contentType.includes("application/json")) {
      const body = (await response.json()) as { error?: unknown };
      if (typeof body.error === "string" && body.error.trim()) {
        code = body.error;
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
  return new ApiResponseError(response.status, code);
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
