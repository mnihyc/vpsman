import { chacha20poly1305 } from "@noble/ciphers/chacha.js";
import { x25519 } from "@noble/curves/ed25519.js";
import { createHash } from "node:crypto";

export function buildEncryptedBackupArtifactFixture(privateKeyHex: string, clientId: string) {
  const archive = {
    client_id: clientId,
    created_unix: 1_780_000_000,
    files: [
      {
        data_base64: Buffer.from("edge-sfo-01\n").toString("base64"),
        mode: 0o644,
        path: "/etc/hostname",
        sha256_hex: sha256Hex(Buffer.from("edge-sfo-01\n")),
        size_bytes: 12,
        source: "selected_path",
      },
    ],
    format: "vpsman.backup_archive.v1",
  };
  const archiveBytes = new TextEncoder().encode(JSON.stringify(archive));
  const compressedArchive = lz4SizePrependedLiteralOnly(archiveBytes);
  const privateKey = Buffer.from(privateKeyHex, "hex");
  const recipientPublic = x25519.getPublicKey(privateKey);
  const ephemeralSecret = new Uint8Array(32).fill(9);
  const ephemeralPublic = x25519.getPublicKey(ephemeralSecret);
  const sharedSecret = x25519.scalarMult(ephemeralSecret, recipientPublic);
  const key = sha256Bytes(
    concatBytes([
      new TextEncoder().encode("vpsman-backup-artifact-v1"),
      sharedSecret,
      recipientPublic,
      ephemeralPublic,
    ]),
  );
  const nonce = new Uint8Array(12).fill(3);
  const ciphertext = chacha20poly1305(key, nonce).encrypt(compressedArchive);
  const artifact = {
    cipher: "x25519-chacha20poly1305",
    ciphertext_base64: Buffer.from(ciphertext).toString("base64"),
    ciphertext_sha256_hex: sha256Hex(ciphertext),
    client_id: clientId,
    compression: "lz4-size-prepended",
    created_unix: archive.created_unix,
    ephemeral_public_key_hex: Buffer.from(ephemeralPublic).toString("hex"),
    format: "vpsman.backup_artifact.v1",
    nonce_hex: Buffer.from(nonce).toString("hex"),
    recipient_public_key_sha256_hex: sha256Hex(recipientPublic),
    version: 1,
  };
  return {
    archiveBytes,
    archiveSha256Hex: sha256Hex(archiveBytes),
    artifact,
  };
}

function lz4SizePrependedLiteralOnly(plaintext: Uint8Array): Uint8Array {
  const lengthBytes: number[] = [];
  let remaining = plaintext.length - 15;
  if (plaintext.length >= 15) {
    while (remaining >= 255) {
      lengthBytes.push(255);
      remaining -= 255;
    }
    lengthBytes.push(remaining);
  }
  const output = new Uint8Array(4 + 1 + lengthBytes.length + plaintext.length);
  const view = new DataView(output.buffer);
  view.setUint32(0, plaintext.length, true);
  output[4] = Math.min(plaintext.length, 15) << 4;
  output.set(lengthBytes, 5);
  output.set(plaintext, 5 + lengthBytes.length);
  return output;
}

export function sha256Hex(bytes: Uint8Array): string {
  return createHash("sha256").update(bytes).digest("hex");
}

function sha256Bytes(bytes: Uint8Array): Uint8Array {
  return new Uint8Array(createHash("sha256").update(bytes).digest());
}

function concatBytes(parts: Uint8Array[]): Uint8Array {
  const output = new Uint8Array(parts.reduce((total, part) => total + part.length, 0));
  let offset = 0;
  for (const part of parts) {
    output.set(part, offset);
    offset += part.length;
  }
  return output;
}
