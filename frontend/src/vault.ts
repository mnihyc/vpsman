import { normalizeHex, type ProofMaterial } from "./proof";
import type { AuthResponse, OperatorView } from "./types";

const encoder = new TextEncoder();
const decoder = new TextDecoder();
const PROOF_STORAGE_KEY = "vpsman.proofVault";
const AUTH_STORAGE_KEY = "vpsman.authVault";
const KDF_ITERATIONS = 180_000;

type StoredEncryptedVault = {
  version: 1;
  kdf: "PBKDF2-SHA256";
  iterations: number;
  salt_hex: string;
  cipher: "AES-GCM";
  iv_hex: string;
  ciphertext_hex: string;
};

type StoredAuthVaultPayload = {
  token_type: "Bearer";
  access_token: string;
  refresh_token: string;
  expires_in_secs: number;
  refresh_expires_in_secs: number;
  operator: OperatorView;
};

export function hasProofVault(): boolean {
  return window.localStorage.getItem(PROOF_STORAGE_KEY) !== null;
}

export function clearProofVault(): void {
  window.localStorage.removeItem(PROOF_STORAGE_KEY);
}

export function hasAuthVault(): boolean {
  return window.localStorage.getItem(AUTH_STORAGE_KEY) !== null;
}

export function clearAuthVault(): void {
  window.localStorage.removeItem(AUTH_STORAGE_KEY);
}

export async function saveProofVault(material: ProofMaterial, passphrase: string): Promise<void> {
  if (!passphrase) {
    throw new Error("Vault passphrase is required");
  }
  if (!material.superPassword) {
    throw new Error("Super password is required");
  }
  const normalizedMaterial = {
    superPassword: material.superPassword,
    superSaltHex: normalizeHex(material.superSaltHex),
  };
  const record = await encryptVaultPayload(normalizedMaterial, passphrase);
  window.localStorage.setItem(PROOF_STORAGE_KEY, JSON.stringify(record));
}

export async function loadProofVault(passphrase: string): Promise<ProofMaterial> {
  if (!passphrase) {
    throw new Error("Vault passphrase is required");
  }
  const raw = window.localStorage.getItem(PROOF_STORAGE_KEY);
  if (!raw) {
    throw new Error("No proof vault exists");
  }
  const material = await decryptVaultPayload<Partial<ProofMaterial>>(raw, passphrase);
  if (typeof material.superPassword !== "string" || typeof material.superSaltHex !== "string") {
    throw new Error("Proof vault payload is invalid");
  }
  return {
    superPassword: material.superPassword,
    superSaltHex: normalizeHex(material.superSaltHex),
  };
}

export async function saveAuthVault(auth: AuthResponse, passphrase: string): Promise<void> {
  if (!passphrase) {
    throw new Error("Session vault key is required");
  }
  const payload: StoredAuthVaultPayload = {
    token_type: "Bearer",
    access_token: auth.access_token,
    refresh_token: auth.refresh_token,
    expires_in_secs: auth.expires_in_secs,
    refresh_expires_in_secs: auth.refresh_expires_in_secs,
    operator: auth.operator,
  };
  validateAuthVaultPayload(payload);
  const record = await encryptVaultPayload(payload, passphrase);
  window.localStorage.setItem(AUTH_STORAGE_KEY, JSON.stringify(record));
}

export async function loadAuthVault(passphrase: string): Promise<AuthResponse> {
  if (!passphrase) {
    throw new Error("Session vault key is required");
  }
  const raw = window.localStorage.getItem(AUTH_STORAGE_KEY);
  if (!raw) {
    throw new Error("No encrypted session vault exists");
  }
  const payload = await decryptVaultPayload<Partial<StoredAuthVaultPayload>>(raw, passphrase);
  validateAuthVaultPayload(payload);
  return payload;
}

async function encryptVaultPayload(payload: unknown, passphrase: string): Promise<StoredEncryptedVault> {
  const kdfSalt = randomBytes(16);
  const iv = randomBytes(12);
  const key = await deriveVaultKey(passphrase, kdfSalt);
  const ciphertext = new Uint8Array(
    await cryptoProvider().subtle.encrypt(
      { name: "AES-GCM", iv: bufferSource(iv) },
      key,
      bufferSource(encoder.encode(JSON.stringify(payload))),
    ),
  );
  return {
    version: 1,
    kdf: "PBKDF2-SHA256",
    iterations: KDF_ITERATIONS,
    salt_hex: bytesToHex(kdfSalt),
    cipher: "AES-GCM",
    iv_hex: bytesToHex(iv),
    ciphertext_hex: bytesToHex(ciphertext),
  };
}

async function decryptVaultPayload<T>(raw: string, passphrase: string): Promise<T> {
  const record = parseVaultRecord(raw);
  const key = await deriveVaultKey(passphrase, hexToBytes(record.salt_hex), record.iterations);
  const plaintext = await cryptoProvider().subtle.decrypt(
    { name: "AES-GCM", iv: bufferSource(hexToBytes(record.iv_hex)) },
    key,
    bufferSource(hexToBytes(record.ciphertext_hex)),
  );
  return JSON.parse(decoder.decode(plaintext)) as T;
}

async function deriveVaultKey(passphrase: string, salt: Uint8Array, iterations = KDF_ITERATIONS): Promise<CryptoKey> {
  const baseKey = await cryptoProvider().subtle.importKey(
    "raw",
    bufferSource(encoder.encode(passphrase)),
    "PBKDF2",
    false,
    ["deriveKey"],
  );
  return cryptoProvider().subtle.deriveKey(
    {
      name: "PBKDF2",
      salt: bufferSource(salt),
      iterations,
      hash: "SHA-256",
    },
    baseKey,
    { name: "AES-GCM", length: 256 },
    false,
    ["encrypt", "decrypt"],
  );
}

function parseVaultRecord(raw: string): StoredEncryptedVault {
  const record = JSON.parse(raw) as Partial<StoredEncryptedVault>;
  if (
    record.version !== 1 ||
    record.kdf !== "PBKDF2-SHA256" ||
    record.cipher !== "AES-GCM" ||
    typeof record.iterations !== "number" ||
    record.iterations < 100_000 ||
    record.iterations > 1_000_000 ||
    typeof record.salt_hex !== "string" ||
    typeof record.iv_hex !== "string" ||
    typeof record.ciphertext_hex !== "string"
  ) {
    throw new Error("Proof vault record is invalid");
  }
  return {
    version: 1,
    kdf: "PBKDF2-SHA256",
    iterations: record.iterations,
    salt_hex: normalizeHex(record.salt_hex),
    cipher: "AES-GCM",
    iv_hex: normalizeHex(record.iv_hex),
    ciphertext_hex: normalizeHex(record.ciphertext_hex),
  };
}

function validateAuthVaultPayload(payload: Partial<StoredAuthVaultPayload>): asserts payload is StoredAuthVaultPayload {
  const operator = payload.operator;
  if (
    !operator ||
    typeof operator.id !== "string" ||
    typeof operator.username !== "string" ||
    typeof operator.role !== "string" ||
    !Array.isArray(operator.scopes)
  ) {
    throw new Error("Session vault payload is invalid");
  }
  if (
    payload.token_type !== "Bearer" ||
    typeof payload.access_token !== "string" ||
    !/^[0-9a-f]{64}$/i.test(payload.access_token) ||
    typeof payload.refresh_token !== "string" ||
    !/^[0-9a-f]{64}$/i.test(payload.refresh_token) ||
    typeof payload.expires_in_secs !== "number" ||
    typeof payload.refresh_expires_in_secs !== "number"
  ) {
    throw new Error("Session vault payload is invalid");
  }
}

function randomBytes(length: number): Uint8Array {
  return cryptoProvider().getRandomValues(new Uint8Array(length));
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

function bufferSource(bytes: Uint8Array): ArrayBuffer {
  return bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength) as ArrayBuffer;
}

function cryptoProvider(): Crypto {
  if (!globalThis.crypto?.subtle) {
    throw new Error("WebCrypto is unavailable");
  }
  return globalThis.crypto;
}
