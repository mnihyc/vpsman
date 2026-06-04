import type { CommandEnvelope, JobOperation } from "./types";

const encoder = new TextEncoder();
const SUPER_KEY_DOMAIN = "vpsman-super-key-v1";
const COMMAND_PROOF_DOMAIN = "vpsman-privileged-command-v1";

export type ProofMaterial = {
  superPassword: string;
  superSaltHex: string;
};

export type BuiltCommandEnvelopes = {
  payloadHashHex: string;
  envelopes: Record<string, CommandEnvelope>;
};

export function parseCommandArgv(input: string): string[] {
  const argv: string[] = [];
  let current = "";
  let quote: "'" | "\"" | null = null;
  let escaping = false;

  for (const char of input.trim()) {
    if (escaping) {
      current += char;
      escaping = false;
      continue;
    }
    if (char === "\\") {
      escaping = true;
      continue;
    }
    if (quote) {
      if (char === quote) {
        quote = null;
      } else {
        current += char;
      }
      continue;
    }
    if (char === "'" || char === "\"") {
      quote = char;
      continue;
    }
    if (/\s/.test(char)) {
      if (current) {
        argv.push(current);
        current = "";
      }
      continue;
    }
    current += char;
  }

  if (escaping) {
    current += "\\";
  }
  if (quote) {
    throw new Error("Unterminated quoted argument");
  }
  if (current) {
    argv.push(current);
  }
  return argv;
}

export async function buildCommandEnvelopesForClients({
  argv,
  clientIds,
  proofTtlSecs,
  superPassword,
  superSaltHex,
}: {
  argv: string[];
  clientIds: string[];
  proofTtlSecs: number;
  superPassword: string;
  superSaltHex: string;
}): Promise<BuiltCommandEnvelopes> {
  if (argv.length === 0 || argv.some((part) => part.length === 0)) {
    throw new Error("Command argv is empty");
  }
  return buildEnvelopesForOperation({
    clientIds,
    operation: { type: "shell", argv, pty: false },
    proofTtlSecs,
    superPassword,
    superSaltHex,
  });
}

export async function buildEnvelopesForOperation({
  clientIds,
  maxProofTtlSecs,
  operation,
  proofTtlSecs,
  superPassword,
  superSaltHex,
}: {
  clientIds: string[];
  maxProofTtlSecs?: number;
  operation: JobOperation;
  proofTtlSecs: number;
  superPassword: string;
  superSaltHex: string;
}): Promise<BuiltCommandEnvelopes> {
  if (clientIds.length === 0) {
    throw new Error("No resolved clients");
  }

  const payload = operationPayloadBytes(operation);
  const payloadHashHex = await sha256Hex(payload);
  const envelopes = await buildEnvelopeMap({
    clientIds,
    maxProofTtlSecs,
    payloadHashHex,
    proofTtlSecs,
    superPassword,
    superSaltHex,
  });

  return { payloadHashHex, envelopes };
}

export async function buildEnvelopesForPayloadHash({
  clientIds,
  payloadHashHex,
  proofTtlSecs,
  superPassword,
  superSaltHex,
}: {
  clientIds: string[];
  payloadHashHex: string;
  proofTtlSecs: number;
  superPassword: string;
  superSaltHex: string;
}): Promise<BuiltCommandEnvelopes> {
  const normalizedPayloadHashHex = normalizeSha256Hex(payloadHashHex);
  const envelopes = await buildEnvelopeMap({
    clientIds,
    payloadHashHex: normalizedPayloadHashHex,
    proofTtlSecs,
    superPassword,
    superSaltHex,
  });

  return { payloadHashHex: normalizedPayloadHashHex, envelopes };
}

export async function deriveSuperKeyHex(superPassword: string, superSaltHex: string): Promise<string> {
  if (!superPassword) {
    throw new Error("Super password is required");
  }
  const salt = hexToBytes(superSaltHex);
  const keyMaterial = concatBytes([
    encoder.encode(SUPER_KEY_DOMAIN),
    u64Bytes(salt.length),
    salt,
    encoder.encode(superPassword),
  ]);
  const keyBytes = new Uint8Array(await cryptoProvider().subtle.digest("SHA-256", bufferSource(keyMaterial)));
  return bytesToHex(keyBytes);
}

async function buildEnvelopeMap({
  clientIds,
  maxProofTtlSecs = 3600,
  payloadHashHex,
  proofTtlSecs,
  superPassword,
  superSaltHex,
}: {
  clientIds: string[];
  maxProofTtlSecs?: number;
  payloadHashHex: string;
  proofTtlSecs: number;
  superPassword: string;
  superSaltHex: string;
}): Promise<Record<string, CommandEnvelope>> {
  if (clientIds.length === 0) {
    throw new Error("No resolved clients");
  }

  const superKey = await deriveSuperHmacKey(superPassword, superSaltHex);
  const expiresUnix = Math.floor(Date.now() / 1000) + clampInteger(proofTtlSecs, 15, maxProofTtlSecs);
  const envelopes: Record<string, CommandEnvelope> = {};

  for (const clientId of clientIds) {
    const commandId = randomUuid();
    const scope = `client:${clientId}`;
    const nonce = randomBytes(16);
    const proofPayload = concatBytes([
      encoder.encode(COMMAND_PROOF_DOMAIN),
      uuidBytes(commandId),
      encoder.encode(scope),
      encoder.encode(payloadHashHex),
      nonce,
      u64Bytes(expiresUnix),
    ]);
    const proofBytes = new Uint8Array(await cryptoProvider().subtle.sign("HMAC", superKey, bufferSource(proofPayload)));
    envelopes[clientId] = {
      command_id: commandId,
      scope,
      payload_hash_hex: payloadHashHex,
      proof: {
        nonce_hex: bytesToHex(nonce),
        expires_unix: expiresUnix,
        proof_hex: bytesToHex(proofBytes),
      },
      server_signature: [],
    };
  }

  return envelopes;
}

function operationPayloadBytes(operation: JobOperation): Uint8Array {
  return encoder.encode(JSON.stringify(operation));
}

async function deriveSuperHmacKey(superPassword: string, saltHex: string): Promise<CryptoKey> {
  const keyBytes = hexToBytes(await deriveSuperKeyHex(superPassword, saltHex));
  return cryptoProvider().subtle.importKey("raw", bufferSource(keyBytes), { name: "HMAC", hash: "SHA-256" }, false, [
    "sign",
  ]);
}

async function sha256Hex(payload: Uint8Array): Promise<string> {
  const hash = new Uint8Array(await cryptoProvider().subtle.digest("SHA-256", bufferSource(payload)));
  return bytesToHex(hash);
}

export function normalizeHex(value: string): string {
  const normalized = value.trim().toLowerCase();
  if (normalized.length === 0 || normalized.length % 2 !== 0 || /[^0-9a-f]/.test(normalized)) {
    throw new Error("Invalid hex value");
  }
  return normalized;
}

function normalizeSha256Hex(value: string): string {
  const normalized = normalizeHex(value);
  if (normalized.length !== 64) {
    throw new Error("Payload hash must be a SHA-256 hex value");
  }
  return normalized;
}

function hexToBytes(value: string): Uint8Array {
  const normalized = normalizeHex(value);
  const bytes = new Uint8Array(normalized.length / 2);
  for (let index = 0; index < normalized.length; index += 2) {
    bytes[index / 2] = Number.parseInt(normalized.slice(index, index + 2), 16);
  }
  return bytes;
}

function bytesToHex(bytes: Uint8Array): string {
  return Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join("");
}

function uuidBytes(uuid: string): Uint8Array {
  const normalized = uuid.replace(/-/g, "");
  if (normalized.length !== 32) {
    throw new Error("Invalid UUID");
  }
  return hexToBytes(normalized);
}

function randomBytes(length: number): Uint8Array {
  return cryptoProvider().getRandomValues(new Uint8Array(length));
}

function randomUuid(): string {
  const crypto = cryptoProvider();
  if (typeof crypto.randomUUID === "function") {
    return crypto.randomUUID();
  }

  const bytes = randomBytes(16);
  bytes[6] = (bytes[6] & 0x0f) | 0x40;
  bytes[8] = (bytes[8] & 0x3f) | 0x80;
  const hex = bytesToHex(bytes);
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
}

function u64Bytes(value: number): Uint8Array {
  if (!Number.isSafeInteger(value) || value < 0) {
    throw new Error("Invalid u64 value");
  }
  const bytes = new Uint8Array(8);
  const view = new DataView(bytes.buffer);
  view.setUint32(0, Math.floor(value / 0x100000000), false);
  view.setUint32(4, value >>> 0, false);
  return bytes;
}

function concatBytes(parts: Uint8Array[]): Uint8Array {
  const totalLength = parts.reduce((total, part) => total + part.length, 0);
  const output = new Uint8Array(totalLength);
  let offset = 0;
  for (const part of parts) {
    output.set(part, offset);
    offset += part.length;
  }
  return output;
}

function bufferSource(bytes: Uint8Array): ArrayBuffer {
  return bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength) as ArrayBuffer;
}

function clampInteger(value: number, min: number, max: number): number {
  if (!Number.isFinite(value)) {
    return min;
  }
  return Math.trunc(Math.min(Math.max(value, min), max));
}

function cryptoProvider(): Crypto {
  if (!globalThis.crypto?.subtle) {
    throw new Error("WebCrypto is unavailable");
  }
  return globalThis.crypto;
}
