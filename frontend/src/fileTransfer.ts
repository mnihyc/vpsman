import { sha256 } from "@noble/hashes/sha2.js";

export const MAX_INLINE_FILE_PUSH_BYTES = 1024 * 1024;
export const FILE_TRANSFER_CHUNK_BYTES = 64 * 1024;
export const MAX_CHUNKED_FILE_PUSH_BYTES = 8 * 1024 * 1024;
export const BROWSER_FILE_HASH_CHUNK_BYTES = 4 * 1024 * 1024;

export type Sha256Accumulator = {
  update: (bytes: Uint8Array) => Sha256Accumulator;
  digest: () => Uint8Array;
};

export type FilePushChunk = {
  offset: number;
  size_bytes: number;
  sha256_hex: string;
  data_base64: string;
};

export type FilePushPayload = {
  dataBase64: string;
  sha256Hex: string;
  sizeBytes: number;
  chunks?: FilePushChunk[];
};

export async function readFilePushPayload(file: File | null): Promise<FilePushPayload> {
  if (!file) {
    throw new Error("File push source is required");
  }
  if (file.size > MAX_CHUNKED_FILE_PUSH_BYTES) {
    throw new Error(`File push source exceeds ${MAX_CHUNKED_FILE_PUSH_BYTES} bytes`);
  }
  const bytes = new Uint8Array(await file.arrayBuffer());
  const sha256 = await sha256Hex(bytes);
  if (bytes.length <= MAX_INLINE_FILE_PUSH_BYTES) {
    return {
      dataBase64: bytesToBase64(bytes),
      sha256Hex: sha256,
      sizeBytes: bytes.length,
    };
  }
  const chunks: FilePushChunk[] = [];
  for (let offset = 0; offset < bytes.length; offset += FILE_TRANSFER_CHUNK_BYTES) {
    const chunk = bytes.subarray(offset, Math.min(offset + FILE_TRANSFER_CHUNK_BYTES, bytes.length));
    chunks.push({
      offset,
      size_bytes: chunk.length,
      sha256_hex: await sha256Hex(chunk),
      data_base64: bytesToBase64(chunk),
    });
  }
  return {
    dataBase64: "",
    sha256Hex: sha256,
    sizeBytes: bytes.length,
    chunks,
  };
}

export async function sha256Hex(bytes: Uint8Array): Promise<string> {
  const hash = new Uint8Array(await crypto.subtle.digest("SHA-256", bufferSource(bytes)));
  return bytesToHex(hash);
}

export async function sha256FileHex(file: File, chunkSizeBytes = BROWSER_FILE_HASH_CHUNK_BYTES): Promise<string> {
  if (chunkSizeBytes <= 0) {
    throw new Error("File hash chunk size must be positive");
  }
  const hasher = sha256.create();
  for (let offset = 0; offset < file.size; offset += chunkSizeBytes) {
    const chunk = await readFileSlice(file, offset, Math.min(offset + chunkSizeBytes, file.size));
    hasher.update(chunk);
  }
  return bytesToHex(hasher.digest());
}

export function createSha256Accumulator(): Sha256Accumulator {
  return sha256.create();
}

export async function readFileSlice(file: File, start: number, end: number): Promise<Uint8Array> {
  if (start < 0 || end < start || end > file.size) {
    throw new Error("File slice range is invalid");
  }
  return new Uint8Array(await file.slice(start, end).arrayBuffer());
}

export function parseFileMode(value: string): number {
  const trimmed = value.trim();
  if (!trimmed) {
    throw new Error("File mode is required");
  }
  const digits = trimmed.startsWith("0o") ? trimmed.slice(2) : trimmed;
  if (!/^[0-7]{1,4}$/.test(digits)) {
    throw new Error("File mode must be an octal value between 0000 and 0777");
  }
  const mode = Number.parseInt(digits, 8);
  if (!Number.isInteger(mode) || mode < 0 || mode > 0o777) {
    throw new Error("File mode must be between 0000 and 0777");
  }
  return mode;
}

export function bytesToBase64(bytes: Uint8Array): string {
  let binary = "";
  const chunkSize = 8192;
  for (let index = 0; index < bytes.length; index += chunkSize) {
    const chunk = bytes.subarray(index, index + chunkSize);
    binary += String.fromCharCode(...chunk);
  }
  return btoa(binary);
}

export function base64ToBytes(value: string): Uint8Array {
  const binary = atob(value);
  const bytes = new Uint8Array(binary.length);
  for (let index = 0; index < binary.length; index += 1) {
    bytes[index] = binary.charCodeAt(index);
  }
  return bytes;
}

export function bytesToHex(bytes: Uint8Array): string {
  return Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join("");
}

function bufferSource(bytes: Uint8Array): ArrayBuffer {
  return bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength) as ArrayBuffer;
}
