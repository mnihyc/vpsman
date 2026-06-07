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
  return encoder.encode(canonicalOperationJson(operation));
}

export function canonicalOperationJson(operation: JobOperation): string {
  return JSON.stringify(canonicalJobOperation(operation));
}

type JsonValue = string | number | boolean | null | JsonValue[] | { [key: string]: JsonValue };

function canonicalJobOperation(operation: JobOperation): JsonValue {
  switch (operation.type) {
    case "shell":
      return ordered([["type", operation.type], ["argv", operation.argv], ["pty", operation.pty]]);
    case "shell_script":
      return ordered([["type", operation.type], ["script", operation.script]]);
    case "terminal_open":
      return ordered([
        ["type", operation.type],
        ["session_id", operation.session_id],
        ["argv", operation.argv],
        ["cwd", optional(operation.cwd)],
        ["cols", operation.cols],
        ["rows", operation.rows],
        ["replay_from_seq", optional(operation.replay_from_seq)],
        ["idle_timeout_secs", operation.idle_timeout_secs ?? 1800],
        ["flow_window_bytes", operation.flow_window_bytes ?? 64 * 1024],
      ]);
    case "terminal_input":
      return ordered([
        ["type", operation.type],
        ["session_id", operation.session_id],
        ["input_seq", operation.input_seq],
        ["data_base64", operation.data_base64],
      ]);
    case "terminal_poll":
      return ordered([["type", operation.type], ["session_id", operation.session_id], ["replay_from_seq", optional(operation.replay_from_seq)]]);
    case "terminal_resize":
      return ordered([["type", operation.type], ["session_id", operation.session_id], ["cols", operation.cols], ["rows", operation.rows]]);
    case "terminal_close":
      return ordered([["type", operation.type], ["session_id", operation.session_id], ["reason", optional(operation.reason)]]);
    case "file_pull":
    case "file_stat":
      return ordered([["type", operation.type], ["path", operation.path]]);
    case "hot_config":
    case "data_source_config_patch":
      return ordered([["type", operation.type], ["toml", operation.toml]]);
    case "auth_proof_key_rotate":
      return ordered([
        ["type", operation.type],
        ["new_proof_key_hex", operation.new_proof_key_hex],
        ["rotation_generation", optional(operation.rotation_generation)],
      ]);
    case "agent_update":
      return ordered([
        ["type", operation.type],
        ["artifact_url", operation.artifact_url],
        ["sha256_hex", operation.sha256_hex],
        ["artifact_signature_hex", optional(operation.artifact_signature_hex)],
        ["artifact_signing_key_hex", optional(operation.artifact_signing_key_hex)],
      ]);
    case "agent_update_activate":
      return ordered([
        ["type", operation.type],
        ["staged_sha256_hex", operation.staged_sha256_hex],
        ["restart_agent", skipFalse(operation.restart_agent)],
      ]);
    case "agent_update_rollback":
      return ordered([["type", operation.type], ["rollback_sha256_hex", optional(operation.rollback_sha256_hex)]]);
    case "agent_update_check":
      return ordered([
        ["type", operation.type],
        ["version_url", optional(operation.version_url)],
        ["activate", operation.activate ?? true],
        ["restart_agent", operation.restart_agent ?? true],
      ]);
    case "file_push":
      return ordered([
        ["type", operation.type],
        ["path", operation.path],
        ["mode", operation.mode],
        ["size_bytes", operation.size_bytes],
        ["sha256_hex", operation.sha256_hex],
        ["data_base64", operation.data_base64],
        ["existing_policy", skipDefault(operation.existing_policy, "skip")],
        ["owner", optional(operation.owner)],
        ["group", optional(operation.group)],
        ["uid", optional(operation.uid)],
        ["gid", optional(operation.gid)],
        ["ownership_policy", skipDefault(operation.ownership_policy, "fail")],
      ]);
    case "file_push_chunked":
      return ordered([
        ["type", operation.type],
        ["path", operation.path],
        ["mode", operation.mode],
        ["size_bytes", operation.size_bytes],
        ["sha256_hex", operation.sha256_hex],
        ["chunks", operation.chunks.map(canonicalFilePushChunk)],
        ["existing_policy", skipDefault(operation.existing_policy, "skip")],
        ["owner", optional(operation.owner)],
        ["group", optional(operation.group)],
        ["uid", optional(operation.uid)],
        ["gid", optional(operation.gid)],
        ["ownership_policy", skipDefault(operation.ownership_policy, "fail")],
      ]);
    case "file_transfer_start":
      return ordered([
        ["type", operation.type],
        ["session_id", operation.session_id],
        ["path", operation.path],
        ["mode", operation.mode],
        ["size_bytes", operation.size_bytes],
        ["sha256_hex", operation.sha256_hex],
        ["chunk_size_bytes", operation.chunk_size_bytes],
        ["rate_limit_kbps", operation.rate_limit_kbps],
        ["existing_policy", skipDefault(operation.existing_policy, "skip")],
        ["resume_token_hash", operation.resume_token_hash],
      ]);
    case "file_transfer_chunk":
      return ordered([
        ["type", operation.type],
        ["session_id", operation.session_id],
        ["offset", operation.offset],
        ["chunk", canonicalFilePushChunk(operation.chunk)],
        ["resume_token_hash", operation.resume_token_hash],
      ]);
    case "file_transfer_commit":
    case "file_transfer_abort":
      return ordered([["type", operation.type], ["session_id", operation.session_id], ["resume_token_hash", operation.resume_token_hash]]);
    case "file_transfer_download_start":
      return ordered([
        ["type", operation.type],
        ["session_id", operation.session_id],
        ["path", operation.path],
        ["chunk_size_bytes", operation.chunk_size_bytes],
        ["rate_limit_kbps", operation.rate_limit_kbps],
        ["resume_token_hash", operation.resume_token_hash],
      ]);
    case "file_transfer_download_chunk":
      return ordered([
        ["type", operation.type],
        ["session_id", operation.session_id],
        ["offset", operation.offset],
        ["max_bytes", operation.max_bytes],
        ["resume_token_hash", operation.resume_token_hash],
      ]);
    case "file_list_dir":
      return ordered([
        ["type", operation.type],
        ["path", operation.path],
        ["offset", operation.offset ?? 0],
        ["limit", operation.limit ?? 250],
        ["show_hidden", operation.show_hidden ?? false],
      ]);
    case "file_read_text":
      return ordered([["type", operation.type], ["path", operation.path], ["max_bytes", operation.max_bytes ?? 1024 * 1024]]);
    case "file_write_text":
      return ordered([
        ["type", operation.type],
        ["path", operation.path],
        ["mode", operation.mode],
        ["size_bytes", operation.size_bytes],
        ["sha256_hex", operation.sha256_hex],
        ["content_base64", operation.content_base64],
        ["expected_sha256_hex", optional(operation.expected_sha256_hex)],
        ["create", operation.create ?? false],
        ["policy", operation.policy ?? "fail"],
      ]);
    case "file_mkdir":
      return ordered([["type", operation.type], ["path", operation.path], ["mode", operation.mode], ["recursive", operation.recursive ?? false], ["policy", operation.policy ?? "fail"]]);
    case "file_rename":
      return ordered([["type", operation.type], ["path", operation.path], ["new_path", operation.new_path], ["overwrite", operation.overwrite ?? false], ["policy", operation.policy ?? "fail"]]);
    case "file_delete":
      return ordered([["type", operation.type], ["path", operation.path], ["recursive", operation.recursive ?? false], ["policy", operation.policy ?? "fail"]]);
    case "file_chmod":
      return ordered([["type", operation.type], ["path", operation.path], ["mode", operation.mode], ["recursive", operation.recursive ?? false], ["policy", operation.policy ?? "fail"]]);
    case "file_chown":
      return ordered([
        ["type", operation.type],
        ["path", operation.path],
        ["owner", optional(operation.owner)],
        ["group", optional(operation.group)],
        ["uid", optional(operation.uid)],
        ["gid", optional(operation.gid)],
        ["recursive", operation.recursive ?? false],
        ["ownership_policy", skipDefault(operation.ownership_policy, "fail")],
        ["policy", operation.policy ?? "fail"],
      ]);
    case "file_copy":
      return ordered([
        ["type", operation.type],
        ["path", operation.path],
        ["new_path", operation.new_path],
        ["overwrite", operation.overwrite ?? false],
        ["recursive", operation.recursive ?? false],
        ["policy", operation.policy ?? "fail"],
      ]);
    case "file_download":
      return ordered([["type", operation.type], ["path", operation.path], ["max_bytes", operation.max_bytes ?? 64 * 1024 * 1024]]);
    case "file_archive_tar":
      return ordered([["type", operation.type], ["path", operation.path], ["max_bytes", operation.max_bytes ?? 64 * 1024 * 1024]]);
    case "user_sessions":
      return ordered([["type", operation.type]]);
    case "process_list":
      return ordered([["type", operation.type], ["limit", operation.limit]]);
    case "process_start":
      return ordered([
        ["type", operation.type],
        ["name", operation.name],
        ["argv", operation.argv],
        ["cwd", operation.cwd ?? null],
        ["env", sortedRecord(operation.env)],
        ["policy", canonicalProcessPolicy(operation.policy)],
        ["limits", canonicalProcessLimits(operation.limits)],
      ]);
    case "process_stop":
    case "process_restart":
      return ordered([["type", operation.type], ["name", operation.name]]);
    case "process_status":
      return ordered([["type", operation.type], ["name", operation.name ?? null]]);
    case "process_logs":
      return ordered([["type", operation.type], ["name", operation.name], ["max_bytes", operation.max_bytes]]);
    case "backup":
      return ordered([
        ["type", operation.type],
        ["paths", operation.paths],
        ["include_config", operation.include_config],
        ["recipient_public_key_hex", optional(operation.recipient_public_key_hex)],
      ]);
    case "restore":
      return ordered([
        ["type", operation.type],
        ["source_backup_request_id", operation.source_backup_request_id],
        ["paths", operation.paths],
        ["include_config", operation.include_config],
        ["destination_root", operation.destination_root ?? null],
        ["archive_path", optional(operation.archive_path)],
        ["archive_base64", operation.archive_base64 ?? null],
        ["archive_size_bytes", operation.archive_size_bytes ?? null],
        ["archive_sha256_hex", operation.archive_sha256_hex ?? null],
        ["dry_run", skipFalse(operation.dry_run)],
        ["post_restore_argv", operation.post_restore_argv?.length ? operation.post_restore_argv : undefined],
      ]);
    case "restore_rollback":
      return ordered([
        ["type", operation.type],
        ["source_restore_job_id", operation.source_restore_job_id],
        ["restored_files", operation.restored_files.map(canonicalRestoreRollbackFile)],
      ]);
    case "network_apply":
      return ordered([
        ["type", operation.type],
        ["plan", operation.plan as JsonValue],
        ["side", operation.side],
        ["config_backend", operation.config_backend ?? "ifupdown"],
        ["config_sha256_hex", optional(operation.config_sha256_hex)],
        ["ifupdown_sha256_hex", operation.ifupdown_sha256_hex],
        ["bird2_sha256_hex", operation.bird2_sha256_hex],
      ]);
    case "network_ospf_cost_update":
      return ordered([
        ["type", operation.type],
        ["plan", operation.plan as JsonValue],
        ["side", operation.side],
        ["current_ospf_cost", operation.current_ospf_cost],
        ["recommended_ospf_cost", operation.recommended_ospf_cost],
        ["bird2_sha256_hex", operation.bird2_sha256_hex],
      ]);
    case "network_rollback":
    case "network_status":
      return ordered([["type", operation.type], ["plan", operation.plan as JsonValue], ["side", operation.side]]);
    case "network_probe":
      return ordered([["type", operation.type], ["plan", operation.plan as JsonValue], ["side", operation.side], ["count", operation.count], ["interval_ms", operation.interval_ms]]);
    case "network_speed_test":
      return ordered([
        ["type", operation.type],
        ["plan", operation.plan as JsonValue],
        ["server_side", operation.server_side],
        ["duration_secs", operation.duration_secs],
        ["max_bytes", operation.max_bytes],
        ["rate_limit_kbps", operation.rate_limit_kbps],
        ["port", operation.port],
        ["connect_timeout_ms", operation.connect_timeout_ms],
      ]);
  }
}

function ordered(entries: Array<[string, JsonValue | undefined]>): JsonValue {
  const value: { [key: string]: JsonValue } = {};
  for (const [key, item] of entries) {
    if (item !== undefined) {
      value[key] = item;
    }
  }
  return value;
}

function optional(value: JsonValue | null | undefined): JsonValue | undefined {
  return value === null || value === undefined ? undefined : value;
}

function skipFalse(value: boolean | undefined): boolean | undefined {
  return value ? true : undefined;
}

function skipDefault<T extends JsonValue>(value: T | null | undefined, defaultValue: T): T | undefined {
  const actual = value ?? defaultValue;
  return actual === defaultValue ? undefined : actual;
}

function canonicalFilePushChunk(chunk: { offset: number; size_bytes: number; sha256_hex: string; data_base64: string }): JsonValue {
  return ordered([
    ["offset", chunk.offset],
    ["size_bytes", chunk.size_bytes],
    ["sha256_hex", chunk.sha256_hex],
    ["data_base64", chunk.data_base64],
  ]);
}

function canonicalProcessPolicy(
  policy: Extract<JobOperation, { type: "process_start" }>["policy"],
): JsonValue | undefined {
  const restart = policy?.restart ?? "never";
  const restartMaxRetries = policy?.restart_max_retries ?? 0;
  const restartBackoffSecs = policy?.restart_backoff_secs ?? 5;
  const gracefulStopSecs = policy?.graceful_stop_secs ?? 5;
  if (restart === "never" && restartMaxRetries === 0 && restartBackoffSecs === 5 && gracefulStopSecs === 5) {
    return undefined;
  }
  return ordered([
    ["restart", restart],
    ["restart_max_retries", restartMaxRetries],
    ["restart_backoff_secs", restartBackoffSecs],
    ["graceful_stop_secs", gracefulStopSecs],
  ]);
}

function canonicalProcessLimits(
  limits: Extract<JobOperation, { type: "process_start" }>["limits"],
): JsonValue | undefined {
  const value = ordered([
    ["memory_max_bytes", optional(limits?.memory_max_bytes)],
    ["pids_max", optional(limits?.pids_max)],
    ["open_files_max", optional(limits?.open_files_max)],
    ["cpu_shares", optional(limits?.cpu_shares)],
    ["no_new_privileges", skipFalse(limits?.no_new_privileges)],
  ]) as { [key: string]: JsonValue };
  return Object.keys(value).length === 0 ? undefined : value;
}

function canonicalRestoreRollbackFile(file: Extract<JobOperation, { type: "restore_rollback" }>["restored_files"][number]): JsonValue {
  return ordered([
    ["archive_path", file.archive_path],
    ["destination_path", file.destination_path],
    ["rollback_path", file.rollback_path ?? null],
    ["restored_size_bytes", file.restored_size_bytes],
    ["restored_sha256_hex", file.restored_sha256_hex],
  ]);
}

function sortedRecord(record: Record<string, string>): JsonValue {
  return Object.fromEntries(Object.entries(record).sort(([left], [right]) => left.localeCompare(right)));
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
