import { ApiUnauthorizedError, buildAuthHeaders } from "./api";
import { bytesToHex, createSha256Accumulator } from "./fileTransfer";

export type ArtifactDownloadMode = "browser-download" | "stream-to-file";

export type VerifiedArtifactDownloadRequest = {
  apiToken: string;
  expectedSha256Hex?: string | null;
  expectedSizeBytes?: number | null;
  fileName: string;
  mode: ArtifactDownloadMode;
  path: string;
};

type SaveFilePickerWindow = Window & {
  showSaveFilePicker?: (options?: { suggestedName?: string }) => Promise<{
    createWritable: () => Promise<{
      abort?: (reason?: unknown) => Promise<void>;
      close: () => Promise<void>;
      write: (chunk: Uint8Array) => Promise<void>;
    }>;
  }>;
};

export async function downloadVerifiedArtifact(request: VerifiedArtifactDownloadRequest): Promise<void> {
  const response = await fetch(request.path, { headers: buildAuthHeaders(request.apiToken) });
  if (response.status === 401) {
    throw new ApiUnauthorizedError();
  }
  if (!response.ok) {
    throw new Error(`API ${response.status}`);
  }
  const expectedSha256Hex = expectedHash(request.expectedSha256Hex, response);
  const expectedSizeBytes = expectedSize(request.expectedSizeBytes, response);
  if (request.mode === "stream-to-file") {
    await streamResponseToFile(response, request.fileName, expectedSha256Hex, expectedSizeBytes);
    return;
  }
  await streamResponseToBrowserDownload(response, request.fileName, expectedSha256Hex, expectedSizeBytes);
}

function expectedHash(value: string | null | undefined, response: Response): string {
  const candidate = value?.trim() || response.headers.get("x-vpsman-artifact-sha256")?.trim() || "";
  if (!/^[a-fA-F0-9]{64}$/.test(candidate)) {
    throw new Error("Artifact response is missing SHA-256 metadata");
  }
  return candidate.toLowerCase();
}

function expectedSize(value: number | null | undefined, response: Response): number {
  if (Number.isFinite(value) && value !== null && value !== undefined && value >= 0) {
    return Math.trunc(value);
  }
  const contentLength = Number.parseInt(response.headers.get("content-length") ?? "", 10);
  if (Number.isFinite(contentLength) && contentLength >= 0) {
    return contentLength;
  }
  throw new Error("Artifact response is missing size metadata");
}

async function streamResponseToFile(
  response: Response,
  fileName: string,
  expectedSha256Hex: string,
  expectedSizeBytes: number,
): Promise<void> {
  const picker = (window as SaveFilePickerWindow).showSaveFilePicker;
  if (!picker || !response.body) {
    throw new Error("Stream-to-file artifact download requires File System Access API support");
  }
  const handle = await picker({ suggestedName: fileName });
  const writable = await handle.createWritable();
  try {
    await readVerifiedResponse(response, expectedSha256Hex, expectedSizeBytes, async (chunk) => {
      await writable.write(chunk);
    });
    await writable.close();
  } catch (error) {
    if (writable.abort) {
      await writable.abort(error);
    }
    throw error;
  }
}

async function streamResponseToBrowserDownload(
  response: Response,
  fileName: string,
  expectedSha256Hex: string,
  expectedSizeBytes: number,
): Promise<void> {
  const chunks: Uint8Array[] = [];
  await readVerifiedResponse(response, expectedSha256Hex, expectedSizeBytes, async (chunk) => {
    chunks.push(chunk);
  });
  saveBrowserDownload(concatenateChunks(chunks, expectedSizeBytes), fileName);
}

async function readVerifiedResponse(
  response: Response,
  expectedSha256Hex: string,
  expectedSizeBytes: number,
  onChunk: (chunk: Uint8Array) => Promise<void>,
): Promise<void> {
  const hasher = createSha256Accumulator();
  let receivedBytes = 0;
  if (!response.body) {
    const bytes = new Uint8Array(await response.arrayBuffer());
    hasher.update(bytes);
    receivedBytes = bytes.byteLength;
    await onChunk(bytes);
  } else {
    const reader = response.body.getReader();
    while (true) {
      const { done, value } = await reader.read();
      if (done) {
        break;
      }
      const chunk = value instanceof Uint8Array ? value : new Uint8Array(value);
      hasher.update(chunk);
      receivedBytes += chunk.byteLength;
      await onChunk(chunk);
    }
  }
  if (receivedBytes !== expectedSizeBytes) {
    throw new Error(`Artifact byte count mismatch: got ${receivedBytes}, expected ${expectedSizeBytes}`);
  }
  const actualSha256Hex = bytesToHex(hasher.digest());
  if (actualSha256Hex !== expectedSha256Hex) {
    throw new Error("Artifact SHA-256 mismatch");
  }
}

function concatenateChunks(chunks: Uint8Array[], sizeBytes: number): Uint8Array {
  const bytes = new Uint8Array(sizeBytes);
  let offset = 0;
  for (const chunk of chunks) {
    bytes.set(chunk, offset);
    offset += chunk.byteLength;
  }
  if (offset !== sizeBytes) {
    throw new Error(`Artifact byte count mismatch: got ${offset}, expected ${sizeBytes}`);
  }
  return bytes;
}

function saveBrowserDownload(bytes: Uint8Array, fileName: string) {
  const body = bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength) as ArrayBuffer;
  const blob = new Blob([body], { type: "application/octet-stream" });
  const url = URL.createObjectURL(blob);
  const link = document.createElement("a");
  link.href = url;
  link.download = fileName;
  document.body.appendChild(link);
  link.click();
  link.remove();
  URL.revokeObjectURL(url);
}
