import { chacha20poly1305 } from "@noble/ciphers/chacha.js";
import { x25519 } from "@noble/curves/ed25519.js";
import { bytesToBase64, MAX_INLINE_FILE_PUSH_BYTES, sha256Hex } from "../fileTransfer";
import { normalizeHex } from "../proof";

const BACKUP_ARTIFACT_FORMAT = "vpsman.backup_artifact.v1";
const BACKUP_ARCHIVE_FORMAT = "vpsman.backup_archive.v1";
const BACKUP_ARTIFACT_CIPHER = "x25519-chacha20poly1305";
const BACKUP_ARTIFACT_COMPRESSION = "lz4-size-prepended";
const BACKUP_ENCRYPTION_DOMAIN = new TextEncoder().encode("vpsman-backup-artifact-v1");
const MAX_BACKUP_ARTIFACT_BYTES = 16 * 1024 * 1024;

type EncryptedBackupArtifact = {
  format: typeof BACKUP_ARTIFACT_FORMAT;
  version: 1;
  cipher: typeof BACKUP_ARTIFACT_CIPHER;
  compression: typeof BACKUP_ARTIFACT_COMPRESSION;
  client_id: string;
  created_unix?: number;
  recipient_public_key_sha256_hex: string;
  ephemeral_public_key_hex: string;
  nonce_hex: string;
  ciphertext_sha256_hex: string;
  ciphertext_base64: string;
};

type BackupArchive = {
  format: typeof BACKUP_ARCHIVE_FORMAT;
  client_id?: string;
  files?: unknown[];
};

export type RestoreArchivePayload = {
  archiveBase64: string;
  archiveSha256Hex: string;
  archiveSizeBytes: number;
  artifactClientId: string;
  fileCount: number;
};

export async function decryptBackupArtifactForRestore(
  artifactBytes: Uint8Array,
  privateKeyHex: string,
): Promise<RestoreArchivePayload> {
  if (artifactBytes.length === 0 || artifactBytes.length > MAX_BACKUP_ARTIFACT_BYTES) {
    throw new Error(`Backup artifact must be between 1 and ${MAX_BACKUP_ARTIFACT_BYTES} bytes`);
  }
  const artifact = parseArtifact(artifactBytes);
  const ciphertext = base64ToBytes(artifact.ciphertext_base64);
  if ((await sha256Hex(ciphertext)) !== artifact.ciphertext_sha256_hex) {
    throw new Error("Backup artifact ciphertext SHA-256 mismatch");
  }

  const privateKey = decodeFixedHex(privateKeyHex, "Backup private key");
  const recipientPublic = x25519.getPublicKey(privateKey);
  if ((await sha256Hex(recipientPublic)) !== artifact.recipient_public_key_sha256_hex) {
    throw new Error("Backup private key does not match artifact recipient");
  }

  const ephemeralPublic = decodeFixedHex(artifact.ephemeral_public_key_hex, "Backup artifact ephemeral public key");
  const nonce = hexToBytes(artifact.nonce_hex);
  if (nonce.length !== 12) {
    throw new Error("Backup artifact nonce must be 12 bytes");
  }

  const sharedSecret = x25519.scalarMult(privateKey, ephemeralPublic);
  const key = await backupEncryptionKey(sharedSecret, recipientPublic, ephemeralPublic);
  const compressedArchive = chacha20poly1305(key, nonce).decrypt(ciphertext);
  const archiveBytes = decompressLz4SizePrepended(compressedArchive, MAX_INLINE_FILE_PUSH_BYTES);
  const archive = parseArchive(archiveBytes);
  const archiveSha256Hex = await sha256Hex(archiveBytes);

  return {
    archiveBase64: bytesToBase64(archiveBytes),
    archiveSha256Hex,
    archiveSizeBytes: archiveBytes.length,
    artifactClientId: artifact.client_id,
    fileCount: archive.files?.length ?? 0,
  };
}

function parseArtifact(bytes: Uint8Array): EncryptedBackupArtifact {
  const raw = JSON.parse(new TextDecoder().decode(bytes)) as Partial<EncryptedBackupArtifact>;
  if (raw.format !== BACKUP_ARTIFACT_FORMAT) {
    throw new Error("Backup artifact format is invalid");
  }
  if (raw.version !== 1) {
    throw new Error("Backup artifact version is invalid");
  }
  if (raw.cipher !== BACKUP_ARTIFACT_CIPHER) {
    throw new Error("Backup artifact cipher is invalid");
  }
  if (raw.compression !== BACKUP_ARTIFACT_COMPRESSION) {
    throw new Error("Backup artifact compression is invalid");
  }
  if (!raw.client_id?.trim()) {
    throw new Error("Backup artifact client id is empty");
  }
  const recipientHash = normalizeSha256Hex(raw.recipient_public_key_sha256_hex, "Backup artifact recipient hash");
  const ciphertextHash = normalizeSha256Hex(raw.ciphertext_sha256_hex, "Backup artifact ciphertext hash");
  if (typeof raw.ephemeral_public_key_hex !== "string" || typeof raw.nonce_hex !== "string") {
    throw new Error("Backup artifact key metadata is invalid");
  }
  if (typeof raw.ciphertext_base64 !== "string" || raw.ciphertext_base64.trim().length === 0) {
    throw new Error("Backup artifact ciphertext is required");
  }
  return {
    format: BACKUP_ARTIFACT_FORMAT,
    version: 1,
    cipher: BACKUP_ARTIFACT_CIPHER,
    compression: BACKUP_ARTIFACT_COMPRESSION,
    client_id: raw.client_id,
    created_unix: raw.created_unix,
    recipient_public_key_sha256_hex: recipientHash,
    ephemeral_public_key_hex: normalizeHex(raw.ephemeral_public_key_hex),
    nonce_hex: normalizeHex(raw.nonce_hex),
    ciphertext_sha256_hex: ciphertextHash,
    ciphertext_base64: raw.ciphertext_base64,
  };
}

function parseArchive(bytes: Uint8Array): BackupArchive {
  const raw = JSON.parse(new TextDecoder().decode(bytes)) as Partial<BackupArchive>;
  if (raw.format !== BACKUP_ARCHIVE_FORMAT) {
    throw new Error("Backup archive format is invalid");
  }
  if (raw.files !== undefined && !Array.isArray(raw.files)) {
    throw new Error("Backup archive files are invalid");
  }
  return {
    format: BACKUP_ARCHIVE_FORMAT,
    client_id: raw.client_id,
    files: raw.files,
  };
}

async function backupEncryptionKey(
  sharedSecret: Uint8Array,
  recipientPublic: Uint8Array,
  ephemeralPublic: Uint8Array,
): Promise<Uint8Array> {
  const material = concatBytes([BACKUP_ENCRYPTION_DOMAIN, sharedSecret, recipientPublic, ephemeralPublic]);
  return new Uint8Array(await crypto.subtle.digest("SHA-256", bufferSource(material)));
}

function decompressLz4SizePrepended(input: Uint8Array, maxOutputBytes: number): Uint8Array {
  if (input.length < 4) {
    throw new Error("Backup archive LZ4 payload is missing size prefix");
  }
  const expectedSize = input[0] | (input[1] << 8) | (input[2] << 16) | (input[3] << 24);
  if (expectedSize <= 0 || expectedSize > maxOutputBytes) {
    throw new Error(`Backup archive plaintext must be between 1 and ${maxOutputBytes} bytes`);
  }
  const output = new Uint8Array(expectedSize);
  const block = input.subarray(4);
  let inputOffset = 0;
  let outputOffset = 0;

  while (inputOffset < block.length) {
    const token = block[inputOffset++];
    let literalLength = token >>> 4;
    ({ length: literalLength, offset: inputOffset } = readLz4Length(block, inputOffset, literalLength, expectedSize));
    if (inputOffset + literalLength > block.length || outputOffset + literalLength > output.length) {
      throw new Error("Backup archive LZ4 literal length is invalid");
    }
    output.set(block.subarray(inputOffset, inputOffset + literalLength), outputOffset);
    inputOffset += literalLength;
    outputOffset += literalLength;

    if (inputOffset === block.length) {
      break;
    }
    if (inputOffset + 2 > block.length) {
      throw new Error("Backup archive LZ4 match offset is truncated");
    }
    const matchOffset = block[inputOffset] | (block[inputOffset + 1] << 8);
    inputOffset += 2;
    if (matchOffset === 0 || matchOffset > outputOffset) {
      throw new Error("Backup archive LZ4 match offset is invalid");
    }
    let matchLength = (token & 0x0f) + 4;
    ({ length: matchLength, offset: inputOffset } = readLz4Length(block, inputOffset, matchLength - 4, expectedSize));
    matchLength += 4;
    if (outputOffset + matchLength > output.length) {
      throw new Error("Backup archive LZ4 match length is invalid");
    }
    for (let index = 0; index < matchLength; index += 1) {
      output[outputOffset + index] = output[outputOffset - matchOffset + index];
    }
    outputOffset += matchLength;
  }

  if (outputOffset !== output.length) {
    throw new Error("Backup archive LZ4 output size mismatch");
  }
  return output;
}

function readLz4Length(input: Uint8Array, offset: number, baseLength: number, maxLength: number) {
  let length = baseLength;
  if (baseLength === 15) {
    let byte = 0;
    do {
      if (offset >= input.length) {
        throw new Error("Backup archive LZ4 length is truncated");
      }
      byte = input[offset++];
      length += byte;
      if (length > maxLength) {
        throw new Error("Backup archive LZ4 length exceeds output bound");
      }
    } while (byte === 255);
  }
  return { length, offset };
}

function decodeFixedHex(value: string, label: string): Uint8Array {
  const bytes = hexToBytes(value);
  if (bytes.length !== 32) {
    throw new Error(`${label} must be 32 bytes`);
  }
  return bytes;
}

function hexToBytes(value: string): Uint8Array {
  const normalized = normalizeHex(value);
  const bytes = new Uint8Array(normalized.length / 2);
  for (let index = 0; index < normalized.length; index += 2) {
    bytes[index / 2] = Number.parseInt(normalized.slice(index, index + 2), 16);
  }
  return bytes;
}

function base64ToBytes(value: string): Uint8Array {
  const binary = atob(value);
  const bytes = new Uint8Array(binary.length);
  for (let index = 0; index < binary.length; index += 1) {
    bytes[index] = binary.charCodeAt(index);
  }
  return bytes;
}

function normalizeSha256Hex(value: string | undefined, label: string): string {
  if (typeof value !== "string") {
    throw new Error(`${label} is required`);
  }
  const normalized = normalizeHex(value);
  if (normalized.length !== 64) {
    throw new Error(`${label} must be a SHA-256 hex value`);
  }
  return normalized;
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
