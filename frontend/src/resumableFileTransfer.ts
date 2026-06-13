import {
  base64ToBytes,
  bytesToHex,
  bytesToBase64,
  createSha256Accumulator,
  parseFileMode,
  readFileSlice,
  sha256FileHex,
  sha256Hex,
  FILE_TRANSFER_CHUNK_BYTES,
} from "./fileTransfer";
import { JOB_TERMINAL_STATUSES } from "./generated/protocolContracts";
import { buildPrivilegeForJobOperation, type PrivilegeMaterial } from "./privilege";
import { selectorExpressionForClientIds } from "./searchExpression";
import type { CreateJobRequest, CreateJobResponse, FileExistingPolicy, JobHistoryRecord, JobOutputRecord, JobOperation } from "./types";

export const MAX_BROWSER_RESUMABLE_DOWNLOAD_BYTES = 128 * 1024 * 1024;
export const MAX_RESUMABLE_FILE_PUSH_BYTES = 1024 * 1024 * 1024;
export const MAX_RESUMABLE_FILE_DOWNLOAD_BYTES = 1024 * 1024 * 1024;
export const MAX_BROWSER_RESUMABLE_UPLOAD_BYTES = MAX_RESUMABLE_FILE_PUSH_BYTES;
export const MAX_BROWSER_STREAMING_RESUMABLE_DOWNLOAD_BYTES = MAX_RESUMABLE_FILE_DOWNLOAD_BYTES;
export const MAX_FILE_TRANSFER_RATE_LIMIT_KBPS = 1_000_000;

export type BrowserTransferMultiTargetPolicy = "same-offset" | "independent-offsets";
export type BrowserDownloadSinkMode = "browser-download" | "stream-to-file";

export type ResumableUploadProgress = {
  event: "ready" | "started" | "chunk" | "committed";
  jobId: string | null;
  multiTargetPolicy: BrowserTransferMultiTargetPolicy;
  nextOffset: number;
  targetOffsets: Record<string, number>;
  sizeBytes: number;
  sessionId: string;
  resumeToken: string;
};

export type ResumableUploadRequest = {
  clientIds: string[];
  confirmed: boolean;
  createJob: (request: CreateJobRequest) => Promise<CreateJobResponse>;
  file: File | null;
  loadJob: (jobId: string) => Promise<JobHistoryRecord>;
  loadOutputs: (jobId: string) => Promise<JobOutputRecord[]>;
  modeText: string;
  multiTargetPolicy?: BrowserTransferMultiTargetPolicy;
  existingPolicy?: FileExistingPolicy;
  path: string;
  privilegeMaterial: PrivilegeMaterial;
  rateLimitKbps: number;
  chunkSizeBytes: number;
  resumeToken?: string;
  sessionId?: string;
  timeoutSecs: number;
  onProgress: (progress: ResumableUploadProgress) => void;
};

export type ResumableDownloadProgress = {
  event: "ready" | "started" | "chunk" | "downloaded";
  downloadSink: BrowserDownloadSinkMode;
  jobId: string | null;
  nextOffset: number;
  sizeBytes: number;
  sessionId: string;
  resumeToken: string;
};

export type ResumableDownloadRequest = {
  clientIds: string[];
  confirmed: boolean;
  createJob: (request: CreateJobRequest) => Promise<CreateJobResponse>;
  downloadName: string;
  downloadSink?: BrowserDownloadSinkMode;
  downloadOutputArtifact: (jobId: string, clientId: string, seq: number) => Promise<Blob>;
  loadJob: (jobId: string) => Promise<JobHistoryRecord>;
  loadOutputs: (jobId: string) => Promise<JobOutputRecord[]>;
  path: string;
  privilegeMaterial: PrivilegeMaterial;
  rateLimitKbps: number;
  chunkSizeBytes: number;
  resumeToken?: string;
  sessionId?: string;
  timeoutSecs: number;
  onProgress: (progress: ResumableDownloadProgress) => void;
};

type TransferStatusPayload = {
  type: string;
  session_id: string;
  next_offset: number;
  size_bytes?: number | null;
  extra?: Record<string, unknown>;
};

type TransferClientStatus = {
  clientId: string;
  payload: TransferStatusPayload;
};

type DownloadByteSink = {
  append: (chunk: Uint8Array) => Promise<void>;
  abort: (reason?: unknown) => Promise<void>;
  complete: (expectedSha256Hex: string, sizeBytes: number) => Promise<void>;
};

type SaveFilePickerWindow = Window & {
  showSaveFilePicker?: (options?: { suggestedName?: string }) => Promise<{
    createWritable: () => Promise<{
      write: (chunk: Uint8Array) => Promise<void>;
      close: () => Promise<void>;
      abort?: (reason?: unknown) => Promise<void>;
    }>;
  }>;
};

export async function runBrowserResumableUpload(request: ResumableUploadRequest): Promise<CreateJobResponse> {
  if (!request.confirmed) {
    throw new Error("Resumable upload requires confirmation");
  }
  if (!request.file) {
    throw new Error("Resumable upload source is required");
  }
  const remotePath = normalizeTransferAbsolutePath(request.path, "Resumable upload path");
  if (request.file.size > MAX_RESUMABLE_FILE_PUSH_BYTES) {
    throw new Error(`Resumable upload exceeds ${MAX_RESUMABLE_FILE_PUSH_BYTES} bytes`);
  }
  const chunkSizeBytes = clampInteger(request.chunkSizeBytes, 1, FILE_TRANSFER_CHUNK_BYTES);
  const rateLimitKbps = clampInteger(request.rateLimitKbps, 0, MAX_FILE_TRANSFER_RATE_LIMIT_KBPS);
  const multiTargetPolicy = request.multiTargetPolicy ?? "same-offset";
  const existingPolicy = request.existingPolicy ?? "skip";
  const sessionId = request.sessionId?.trim() || crypto.randomUUID();
  const resumeToken = request.resumeToken?.trim() || randomHex(32);
  const mode = parseFileMode(request.modeText);
  const sha256HexValue = await sha256FileHex(request.file);
  const resumeTokenHash = await sha256Hex(new TextEncoder().encode(resumeToken));
  const sizeBytes = request.file.size;
  const initialTargetOffsets = Object.fromEntries(request.clientIds.map((clientId) => [clientId, 0]));

  request.onProgress({
    event: "ready",
    jobId: null,
    multiTargetPolicy,
    nextOffset: 0,
    resumeToken,
    sessionId,
    sizeBytes,
    targetOffsets: initialTargetOffsets,
  });
  const start = await submitTransferStep(request, "file_transfer_start", {
    type: "file_transfer_start",
    session_id: sessionId,
    path: remotePath,
    mode,
    size_bytes: sizeBytes,
    sha256_hex: sha256HexValue,
    chunk_size_bytes: chunkSizeBytes,
    rate_limit_kbps: rateLimitKbps,
    existing_policy: existingPolicy,
    resume_token_hash: resumeTokenHash,
  });
  const startStatuses = await waitForTransferStatus(
    request,
    start.job_id,
    sessionId,
    "file_transfer_start",
    start.target_count,
    request.clientIds,
  );
  let targetOffsets = targetOffsetsFromStatuses(startStatuses, sizeBytes);
  ensureTargetOffsetsForClients(targetOffsets, request.clientIds, "upload start");
  const activeClientIds = activeClientIdsFromStatuses(startStatuses);
  const activeStartStatuses = startStatuses.filter((status) => activeClientIds.includes(status.clientId));
  if (activeClientIds.length === 0) {
    ensureAllTargetsAtOffset(targetOffsets, sizeBytes, "upload start");
    request.onProgress({
      event: "committed",
      jobId: start.job_id,
      multiTargetPolicy,
      nextOffset: sizeBytes,
      resumeToken,
      sessionId,
      sizeBytes,
      targetOffsets,
    });
    return start;
  }

  if (multiTargetPolicy === "same-offset") {
    let nextOffset = uniformNextOffset(activeStartStatuses, sizeBytes);
    request.onProgress({
      event: "started",
      jobId: start.job_id,
      multiTargetPolicy,
      nextOffset,
      resumeToken,
      sessionId,
      sizeBytes,
      targetOffsets,
    });

    while (nextOffset < sizeBytes) {
      const chunk = await readUploadChunk(request.file, nextOffset, chunkSizeBytes);
      const operation: JobOperation = {
        type: "file_transfer_chunk",
        session_id: sessionId,
        offset: nextOffset,
        chunk: {
          offset: nextOffset,
          size_bytes: chunk.length,
          sha256_hex: await sha256Hex(chunk),
          data_base64: bytesToBase64(chunk),
        },
        resume_token_hash: resumeTokenHash,
      };
      const chunkJob = await submitTransferStep(request, "file_transfer_chunk", operation, activeClientIds);
      const chunkStatuses = await waitForTransferStatus(
        request,
        chunkJob.job_id,
        sessionId,
        "file_transfer_chunk_ack",
        chunkJob.target_count,
        activeClientIds,
      );
      const acknowledgedOffset = uniformNextOffset(chunkStatuses, sizeBytes);
      if (acknowledgedOffset <= nextOffset) {
        throw new Error(`Resumable upload made no progress at offset ${nextOffset}`);
      }
      targetOffsets = { ...targetOffsets, ...targetOffsetsFromStatuses(chunkStatuses, sizeBytes) };
      ensureTargetOffsetsForClients(targetOffsets, request.clientIds, "upload chunk");
      nextOffset = acknowledgedOffset;
      request.onProgress({
        event: "chunk",
        jobId: chunkJob.job_id,
        multiTargetPolicy,
        nextOffset,
        resumeToken,
        sessionId,
        sizeBytes,
        targetOffsets,
      });
    }

    const commit = await submitTransferStep(request, "file_transfer_commit", {
      type: "file_transfer_commit",
      session_id: sessionId,
      resume_token_hash: resumeTokenHash,
    }, activeClientIds);
    const commitStatuses = await waitForTransferStatus(
      request,
      commit.job_id,
      sessionId,
      "file_transfer_commit",
      commit.target_count,
      activeClientIds,
    );
    const committedOffset = uniformNextOffset(commitStatuses, sizeBytes);
    targetOffsets = { ...targetOffsets, ...targetOffsetsFromStatuses(commitStatuses, sizeBytes) };
    ensureTargetOffsetsForClients(targetOffsets, request.clientIds, "upload commit");
    ensureAllTargetsAtOffset(targetOffsets, sizeBytes, "upload commit");
    if (committedOffset !== sizeBytes) {
      throw new Error(`Resumable upload committed ${committedOffset} of ${sizeBytes} bytes`);
    }
    request.onProgress({
      event: "committed",
      jobId: commit.job_id,
      multiTargetPolicy,
      nextOffset: committedOffset,
      resumeToken,
      sessionId,
      sizeBytes,
      targetOffsets,
    });
    return commit;
  }

  request.onProgress({
    event: "started",
    jobId: start.job_id,
    multiTargetPolicy,
    nextOffset: minimumTargetOffset(targetOffsets),
    resumeToken,
    sessionId,
    sizeBytes,
    targetOffsets,
  });

  while (minimumTargetOffset(targetOffsets) < sizeBytes) {
    for (const [offset, targetClientIds] of targetsGroupedByOffset(targetOffsets, sizeBytes)) {
      const chunk = await readUploadChunk(request.file, offset, chunkSizeBytes);
      const operation: JobOperation = {
        type: "file_transfer_chunk",
        session_id: sessionId,
        offset,
        chunk: {
          offset,
          size_bytes: chunk.length,
          sha256_hex: await sha256Hex(chunk),
          data_base64: bytesToBase64(chunk),
        },
        resume_token_hash: resumeTokenHash,
      };
      const chunkJob = await submitTransferStep(request, "file_transfer_chunk", operation, targetClientIds);
      const chunkStatuses = await waitForTransferStatus(
        request,
        chunkJob.job_id,
        sessionId,
        "file_transfer_chunk_ack",
        chunkJob.target_count,
        targetClientIds,
      );
      const chunkTargetOffsets = targetOffsetsFromStatuses(chunkStatuses, sizeBytes);
      ensureTargetOffsetsForClients(chunkTargetOffsets, targetClientIds, "upload chunk");
      for (const clientId of targetClientIds) {
        const acknowledgedOffset = chunkTargetOffsets[clientId];
        if (acknowledgedOffset <= offset) {
          throw new Error(`Resumable upload made no progress for ${clientId} at offset ${offset}`);
        }
        targetOffsets[clientId] = acknowledgedOffset;
      }
      request.onProgress({
        event: "chunk",
        jobId: chunkJob.job_id,
        multiTargetPolicy,
        nextOffset: minimumTargetOffset(targetOffsets),
        resumeToken,
        sessionId,
        sizeBytes,
        targetOffsets: { ...targetOffsets },
      });
    }
  }

  const commit = await submitTransferStep(request, "file_transfer_commit", {
    type: "file_transfer_commit",
    session_id: sessionId,
    resume_token_hash: resumeTokenHash,
  }, activeClientIds);
  const commitStatuses = await waitForTransferStatus(
    request,
    commit.job_id,
    sessionId,
    "file_transfer_commit",
    commit.target_count,
    activeClientIds,
  );
  targetOffsets = { ...targetOffsets, ...targetOffsetsFromStatuses(commitStatuses, sizeBytes) };
  ensureTargetOffsetsForClients(targetOffsets, request.clientIds, "upload commit");
  ensureAllTargetsAtOffset(targetOffsets, sizeBytes, "upload commit");
  request.onProgress({
    event: "committed",
    jobId: commit.job_id,
    multiTargetPolicy,
    nextOffset: sizeBytes,
    resumeToken,
    sessionId,
    sizeBytes,
    targetOffsets,
  });
  return commit;
}

async function readUploadChunk(file: File, offset: number, chunkSizeBytes: number): Promise<Uint8Array> {
  return readFileSlice(file, offset, Math.min(offset + chunkSizeBytes, file.size));
}

export async function runBrowserResumableDownload(request: ResumableDownloadRequest): Promise<CreateJobResponse> {
  if (!request.confirmed) {
    throw new Error("Resumable download requires confirmation");
  }
  const remotePath = normalizeTransferAbsolutePath(request.path, "Resumable download path");
  if (request.clientIds.length !== 1) {
    throw new Error(`Resumable download requires exactly one resolved target; got ${request.clientIds.length}`);
  }
  const chunkSizeBytes = clampInteger(request.chunkSizeBytes, 1, FILE_TRANSFER_CHUNK_BYTES);
  const rateLimitKbps = clampInteger(request.rateLimitKbps, 0, MAX_FILE_TRANSFER_RATE_LIMIT_KBPS);
  const sessionId = request.sessionId?.trim() || crypto.randomUUID();
  const resumeToken = request.resumeToken?.trim() || randomHex(32);
  const resumeTokenHash = await sha256Hex(new TextEncoder().encode(resumeToken));
  const clientId = request.clientIds[0];
  const downloadSinkMode = request.downloadSink ?? "browser-download";

  request.onProgress({ event: "ready", downloadSink: downloadSinkMode, jobId: null, nextOffset: 0, resumeToken, sessionId, sizeBytes: 0 });
  const start = await submitTransferStep(request, "file_transfer_download_start", {
    type: "file_transfer_download_start",
    session_id: sessionId,
    path: remotePath,
    chunk_size_bytes: chunkSizeBytes,
    rate_limit_kbps: rateLimitKbps,
    resume_token_hash: resumeTokenHash,
  });
  const startStatuses = await waitForTransferStatus(request, start.job_id, sessionId, "file_transfer_download_start", start.target_count);
  const sizeBytes = startStatuses[0].payload.size_bytes ?? null;
  if (sizeBytes == null) {
    throw new Error("Resumable download start did not report size");
  }
  if (downloadSinkMode === "browser-download" && sizeBytes > MAX_BROWSER_RESUMABLE_DOWNLOAD_BYTES) {
    throw new Error(
      `Browser download mode is capped at ${MAX_BROWSER_RESUMABLE_DOWNLOAD_BYTES} bytes; select stream-to-file for larger downloads`,
    );
  }
  if (downloadSinkMode === "stream-to-file" && sizeBytes > MAX_BROWSER_STREAMING_RESUMABLE_DOWNLOAD_BYTES) {
    throw new Error(`Stream-to-file download exceeds ${MAX_BROWSER_STREAMING_RESUMABLE_DOWNLOAD_BYTES} bytes`);
  }
  const fileSha256Hex = statusStringExtra(startStatuses[0].payload, "sha256_hex");
  const sink = await openDownloadSink(downloadSinkMode, downloadFileName(request.downloadName, remotePath), sizeBytes);
  let nextOffset = uniformNextOffset(startStatuses, sizeBytes);
  request.onProgress({
    event: "started",
    downloadSink: downloadSinkMode,
    jobId: start.job_id,
    nextOffset,
    resumeToken,
    sessionId,
    sizeBytes,
  });

  try {
    while (nextOffset < sizeBytes) {
      const chunkJob = await submitTransferStep(request, "file_transfer_download_chunk", {
        type: "file_transfer_download_chunk",
        session_id: sessionId,
        offset: nextOffset,
        max_bytes: chunkSizeBytes,
        resume_token_hash: resumeTokenHash,
      });
      const chunkStatuses = await waitForTransferStatus(request, chunkJob.job_id, sessionId, "file_transfer_download_chunk", 1);
      const status = chunkStatuses[0];
      if (status.clientId !== clientId) {
        throw new Error(`Resumable download returned ${status.clientId}, expected ${clientId}`);
      }
      const acknowledgedOffset = uniformNextOffset(chunkStatuses, sizeBytes);
      if (acknowledgedOffset <= nextOffset) {
        throw new Error(`Resumable download made no progress at offset ${nextOffset}`);
      }
      const expectedLength = acknowledgedOffset - nextOffset;
      const chunk = await loadDownloadChunkBytes(request, chunkJob.job_id, clientId, expectedLength);
      const expectedChunkHash = statusStringExtra(status.payload, "chunk_sha256_hex");
      const actualChunkHash = await sha256Hex(chunk);
      if (actualChunkHash !== expectedChunkHash) {
        throw new Error(`Resumable download chunk hash mismatch at offset ${nextOffset}`);
      }
      await sink.append(chunk);
      nextOffset = acknowledgedOffset;
      request.onProgress({
        event: "chunk",
        downloadSink: downloadSinkMode,
        jobId: chunkJob.job_id,
        nextOffset,
        resumeToken,
        sessionId,
        sizeBytes,
      });
    }
    await sink.complete(fileSha256Hex, sizeBytes);
  } catch (error) {
    await sink.abort(error);
    throw error;
  }

  request.onProgress({
    event: "downloaded",
    downloadSink: downloadSinkMode,
    jobId: start.job_id,
    nextOffset: sizeBytes,
    resumeToken,
    sessionId,
    sizeBytes,
  });
  return start;
}

async function submitTransferStep(
  request: ResumableUploadRequest | ResumableDownloadRequest,
  command: string,
  operation: JobOperation,
  targetClientIds: string[] = request.clientIds,
): Promise<CreateJobResponse> {
  const selectorExpression = selectorExpressionForClientIds(targetClientIds);
  const timeoutSecs = clampInteger(request.timeoutSecs, 1, 3600);
  const built = await buildPrivilegeForJobOperation({
    clientIds: targetClientIds,
    commandType: command,
    operation,
    privilegeMaterial: request.privilegeMaterial,
    selectorExpression,
    timeoutSecs,
  });
  return request.createJob({
    argv: [],
    selector_expression: selectorExpression,
    target_client_ids: targetClientIds,
    destructive: false,
    confirmed: request.confirmed,
    command,
    operation,
    timeout_secs: timeoutSecs,
    force_unprivileged: false,
    privileged: true,
    privilege_assertion: built.privilegeAssertion,
  });
}

async function waitForTransferStatus(
  request: ResumableUploadRequest | ResumableDownloadRequest,
  jobId: string,
  sessionId: string,
  expectedType: string,
  expectedTargets: number,
  expectedClientIds?: string[],
): Promise<TransferClientStatus[]> {
  const statuses = new Map<string, TransferClientStatus>();
  const expectedClientSet = expectedClientIds ? new Set(expectedClientIds) : null;
  const expectedStatusCount = expectedClientIds?.length ?? expectedTargets;
  let lastOutputs: JobOutputRecord[] = [];
  for (let poll = 0; poll < 120; poll += 1) {
    lastOutputs = await request.loadOutputs(jobId);
    for (const output of lastOutputs) {
      const payload = parseTransferStatus(output, sessionId, expectedType);
      if (payload && (!expectedClientSet || expectedClientSet.has(output.client_id))) {
        statuses.set(output.client_id, { clientId: output.client_id, payload });
      }
    }
    const job = await request.loadJob(jobId);
    if (isTerminalJobStatus(job.status)) {
      if (job.status !== "completed") {
        throw new Error(`${expectedType} job ${jobId} ended ${job.status}`);
      }
      if (statuses.size !== expectedStatusCount) {
        const missing = expectedClientIds?.filter((clientId) => !statuses.has(clientId)) ?? [];
        const missingText = missing.length > 0 ? `; missing ${missing.join(", ")}` : "";
        throw new Error(`${expectedType} job returned ${statuses.size} of ${expectedStatusCount} ACKs${missingText}`);
      }
      return [...statuses.values()];
    }
    await sleep(250);
  }
  throw new Error(`${expectedType} job ${jobId} did not complete; outputs=${lastOutputs.length}`);
}

function parseTransferStatus(output: JobOutputRecord, sessionId: string, expectedType: string): TransferStatusPayload | null {
  if (output.stream !== "status") {
    return null;
  }
  const text = atob(output.data_base64);
  const value = JSON.parse(text) as Partial<TransferStatusPayload>;
  if (value.type !== expectedType || value.session_id !== sessionId) {
    return null;
  }
  const nextOffset = value.next_offset;
  if (typeof nextOffset !== "number" || !Number.isFinite(nextOffset)) {
    throw new Error("Transfer status missing next offset");
  }
  return {
    type: value.type,
    session_id: value.session_id,
    next_offset: nextOffset,
    size_bytes: value.size_bytes,
    extra: value.extra as Record<string, unknown> | undefined,
  };
}

function uniformNextOffset(statuses: TransferClientStatus[], sizeBytes: number): number {
  if (statuses.length === 0) {
    throw new Error("Transfer step returned no ACKs");
  }
  const nextOffset = statuses[0].payload.next_offset;
  if (nextOffset < 0 || nextOffset > sizeBytes) {
    throw new Error(`Transfer ACK offset ${nextOffset} exceeds ${sizeBytes}`);
  }
  for (const status of statuses) {
    if (status.payload.next_offset !== nextOffset) {
      throw new Error(`Transfer targets diverged at ${status.clientId}`);
    }
    if (status.payload.size_bytes != null && status.payload.size_bytes !== sizeBytes) {
      throw new Error(`Transfer target ${status.clientId} reported mismatched size`);
    }
  }
  return nextOffset;
}

function targetOffsetsFromStatuses(statuses: TransferClientStatus[], sizeBytes: number): Record<string, number> {
  const targetOffsets: Record<string, number> = {};
  for (const status of statuses) {
    const nextOffset = status.payload.next_offset;
    if (nextOffset < 0 || nextOffset > sizeBytes) {
      throw new Error(`Transfer target ${status.clientId} ACK offset ${nextOffset} exceeds ${sizeBytes}`);
    }
    if (status.payload.size_bytes != null && status.payload.size_bytes !== sizeBytes) {
      throw new Error(`Transfer target ${status.clientId} reported mismatched size`);
    }
    targetOffsets[status.clientId] = nextOffset;
  }
  return targetOffsets;
}

function activeClientIdsFromStatuses(statuses: TransferClientStatus[]): string[] {
  return statuses.filter((status) => !transferStatusSkipped(status.payload)).map((status) => status.clientId);
}

function transferStatusSkipped(status: TransferStatusPayload): boolean {
  return status.extra?.skipped === true;
}

function ensureTargetOffsetsForClients(targetOffsets: Record<string, number>, clientIds: string[], label: string) {
  const missing = clientIds.filter((clientId) => targetOffsets[clientId] == null);
  if (missing.length > 0) {
    throw new Error(`${label} missing ACKs for ${missing.join(", ")}`);
  }
}

function ensureAllTargetsAtOffset(targetOffsets: Record<string, number>, expectedOffset: number, label: string) {
  const mismatches = Object.entries(targetOffsets).filter(([, offset]) => offset !== expectedOffset);
  if (mismatches.length > 0) {
    const details = mismatches.map(([clientId, offset]) => `${clientId}:${offset}`).join(", ");
    throw new Error(`${label} expected all targets at ${expectedOffset}; got ${details}`);
  }
}

function minimumTargetOffset(targetOffsets: Record<string, number>): number {
  const offsets = Object.values(targetOffsets);
  if (offsets.length === 0) {
    throw new Error("Transfer has no target offsets");
  }
  return Math.min(...offsets);
}

function targetsGroupedByOffset(targetOffsets: Record<string, number>, sizeBytes: number): Array<[number, string[]]> {
  const groups = new Map<number, string[]>();
  for (const [clientId, offset] of Object.entries(targetOffsets)) {
    if (offset >= sizeBytes) {
      continue;
    }
    const group = groups.get(offset);
    if (group) {
      group.push(clientId);
    } else {
      groups.set(offset, [clientId]);
    }
  }
  return [...groups.entries()].sort(([leftOffset], [rightOffset]) => leftOffset - rightOffset);
}

function clampInteger(value: number, min: number, max: number): number {
  if (!Number.isFinite(value)) {
    return min;
  }
  return Math.trunc(Math.min(Math.max(value, min), max));
}

function normalizeTransferAbsolutePath(path: string, label: string): string {
  const trimmed = path.trim();
  if (!trimmed.startsWith("/")) {
    throw new Error(`${label} must be absolute`);
  }
  const parts: string[] = [];
  for (const part of trimmed.split("/")) {
    if (!part) {
      continue;
    }
    if (part === "." || part === "..") {
      throw new Error(`${label} must not contain . or .. segments`);
    }
    parts.push(part);
  }
  return `/${parts.join("/")}`;
}

function isTerminalJobStatus(status: string): boolean {
  return (JOB_TERMINAL_STATUSES as readonly string[]).includes(status);
}

function randomHex(byteLength: number): string {
  const bytes = new Uint8Array(byteLength);
  crypto.getRandomValues(bytes);
  return Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join("");
}

async function loadDownloadChunkBytes(
  request: ResumableDownloadRequest,
  jobId: string,
  clientId: string,
  expectedLength: number,
): Promise<Uint8Array> {
  const outputs = (await request.loadOutputs(jobId))
    .filter((output) => output.client_id === clientId && output.stream === "stdout")
    .sort((left, right) => left.seq - right.seq);
  if (outputs.length === 0) {
    throw new Error(`Resumable download chunk job ${jobId} returned no stdout`);
  }
  const chunks: Uint8Array[] = [];
  for (const output of outputs) {
    if (output.storage === "object_store") {
      const blob = await request.downloadOutputArtifact(jobId, clientId, output.seq);
      const bytes = new Uint8Array(await blob.arrayBuffer());
      if (output.artifact_sha256_hex && (await sha256Hex(bytes)) !== output.artifact_sha256_hex) {
        throw new Error(`Resumable download artifact hash mismatch for output ${output.seq}`);
      }
      if (output.artifact_size_bytes != null && bytes.length !== output.artifact_size_bytes) {
        throw new Error(`Resumable download artifact size mismatch for output ${output.seq}`);
      }
      chunks.push(bytes);
    } else {
      chunks.push(base64ToBytes(output.data_base64));
    }
  }
  const bytes = concatenateChunks(chunks, expectedLength);
  if (bytes.length !== expectedLength) {
    throw new Error(`Resumable download chunk length mismatch: got ${bytes.length}, expected ${expectedLength}`);
  }
  return bytes;
}

function concatenateChunks(chunks: Uint8Array[], sizeBytes: number): Uint8Array {
  const bytes = new Uint8Array(sizeBytes);
  let offset = 0;
  for (const chunk of chunks) {
    bytes.set(chunk, offset);
    offset += chunk.length;
  }
  if (offset !== sizeBytes) {
    throw new Error(`Transfer byte count mismatch: got ${offset}, expected ${sizeBytes}`);
  }
  return bytes;
}

async function openDownloadSink(mode: BrowserDownloadSinkMode, fileName: string, sizeBytes: number): Promise<DownloadByteSink> {
  if (mode === "stream-to-file") {
    const picker = (window as SaveFilePickerWindow).showSaveFilePicker;
    if (picker) {
      const handle = await picker({ suggestedName: fileName });
      const writable = await handle.createWritable();
      const hasher = createSha256Accumulator();
      let writtenBytes = 0;
      let closed = false;
      return {
        async append(chunk: Uint8Array) {
          hasher.update(chunk);
          writtenBytes += chunk.length;
          await writable.write(chunk);
        },
        async complete(expectedSha256Hex: string, expectedSizeBytes: number) {
          if (writtenBytes !== expectedSizeBytes || writtenBytes !== sizeBytes) {
            throw new Error(`Streamed download byte count mismatch: got ${writtenBytes}, expected ${expectedSizeBytes}`);
          }
          const actualHash = bytesToHex(hasher.digest());
          if (actualHash !== expectedSha256Hex) {
            throw new Error("Resumable download final SHA-256 mismatch");
          }
          await writable.close();
          closed = true;
        },
        async abort(reason?: unknown) {
          if (!closed && writable.abort) {
            await writable.abort(reason);
          }
        },
      };
    }
    if (sizeBytes > MAX_BROWSER_RESUMABLE_DOWNLOAD_BYTES) {
      throw new Error("Stream-to-file download requires a browser with showSaveFilePicker support");
    }
  }
  return openBufferedDownloadSink(fileName);
}

function openBufferedDownloadSink(fileName: string): DownloadByteSink {
  const hasher = createSha256Accumulator();
  const chunks: Uint8Array[] = [];
  let writtenBytes = 0;
  return {
    async append(chunk: Uint8Array) {
      hasher.update(chunk);
      chunks.push(chunk);
      writtenBytes += chunk.length;
    },
    async complete(expectedSha256Hex: string, sizeBytes: number) {
      if (writtenBytes !== sizeBytes) {
        throw new Error(`Transfer byte count mismatch: got ${writtenBytes}, expected ${sizeBytes}`);
      }
      const actualHash = bytesToHex(hasher.digest());
      if (actualHash !== expectedSha256Hex) {
        throw new Error("Resumable download final SHA-256 mismatch");
      }
      saveBrowserDownload(concatenateChunks(chunks, sizeBytes), fileName);
    },
    async abort() {
      chunks.length = 0;
    },
  };
}

function statusStringExtra(payload: TransferStatusPayload, key: string): string {
  const value = payload.extra?.[key];
  if (typeof value !== "string" || !/^[0-9a-fA-F]{64}$/.test(value)) {
    throw new Error(`Transfer status missing ${key}`);
  }
  return value.toLowerCase();
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

function downloadFileName(input: string, remotePath: string): string {
  const parts = remotePath.split("/").filter(Boolean);
  const name = input.trim() || parts[parts.length - 1] || "vpsman-download.bin";
  return name.replace(/[\\/\u0000-\u001f\u007f]+/g, "_").slice(0, 160) || "vpsman-download.bin";
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => window.setTimeout(resolve, ms));
}
