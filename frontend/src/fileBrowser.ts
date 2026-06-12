import { bytesToBase64, parseFileMode, sha256Hex, readFilePushPayload, base64ToBytes } from "./fileTransfer";
import type { FileActionPolicy, FileExistingPolicy, FileOwnershipPolicy, JobOperation, JobOutputRecord, JsonValue } from "./types";
import { decodeOutputPreview, isJsonObject } from "./utils";

export const FILE_BROWSER_LIST_LIMIT = 250;
export const FILE_BROWSER_TEXT_LIMIT_BYTES = 1024 * 1024;
export const FILE_BROWSER_ARCHIVE_LIMIT_BYTES = 16 * 1024 * 1024;

export type FileBrowserEntry = {
  name: string;
  path: string;
  file_type: "directory" | "file" | "other" | "symlink" | string;
  is_dir: boolean;
  is_file: boolean;
  is_symlink: boolean;
  size_bytes: number;
  mode: number;
  uid: number;
  gid: number;
  mtime_unix: number;
  symlink_target: string | null;
};

export type FileListStatus = {
  type: "file_list_dir";
  path: string;
  entries: FileBrowserEntry[];
  total_entries: number;
  truncated: boolean;
  offset: number;
  limit: number;
  metadata: FileBrowserEntry;
};

export type FileReadTextStatus = {
  type: "file_read_text";
  path: string;
  content_base64: string;
  size_bytes: number;
  sha256_hex: string;
  truncated: boolean;
  metadata: FileBrowserEntry;
};

export type FileOperationStatus = {
  type: string;
  path: string;
  status?: string;
  reason?: string;
  new_path?: string;
  sha256_hex?: string;
  size_bytes?: number;
  filename?: string;
  content_type?: string;
  source_kind?: string;
  archive?: boolean;
  hierarchy_sha256_hex?: string;
  content_manifest_sha256_hex?: string;
  manifest_entries?: FileDownloadManifestEntry[];
  manifest_entry_count?: number;
  manifest_emitted_entry_count?: number;
  manifest_truncated?: boolean;
  file_count?: number;
  directory_count?: number;
  symlink_count?: number;
  other_count?: number;
  total_file_bytes?: number;
  overwrite_policy?: FileExistingPolicy;
  ownership_status?: string;
  ownership_reason?: string | null;
};

export type FileDownloadManifestEntry = {
  path: string;
  kind?: string;
  size_bytes?: number;
  sha256_hex?: string;
  symlink_target?: string;
};

export function parentPath(path: string): string {
  const normalized = normalizeAbsolutePath(path);
  if (normalized === "/") {
    return "/";
  }
  const index = normalized.lastIndexOf("/");
  return index <= 0 ? "/" : normalized.slice(0, index);
}

export function joinPath(parent: string, child: string): string {
  const cleanChild = child.trim().replace(/^\/+/, "");
  if (!cleanChild) {
    return normalizeAbsolutePath(parent);
  }
  return normalizeAbsolutePath(`${parent === "/" ? "" : normalizeAbsolutePath(parent)}/${cleanChild}`);
}

export function normalizeAbsolutePath(path: string): string {
  const trimmed = path.trim();
  if (!trimmed.startsWith("/")) {
    throw new Error("Path must be absolute");
  }
  const parts: string[] = [];
  for (const part of trimmed.split("/")) {
    if (!part) {
      continue;
    }
    if (part === "." || part === "..") {
      throw new Error("Path must not contain . or .. segments");
    }
    parts.push(part);
  }
  return `/${parts.join("/")}`;
}

export function safeNormalizeAbsolutePath(path: string, fallback = "/"): string {
  try {
    return normalizeAbsolutePath(path);
  } catch {
    return fallback;
  }
}

export function fileName(path: string): string {
  const normalized = normalizeAbsolutePath(path);
  if (normalized === "/") {
    return "/";
  }
  return normalized.slice(normalized.lastIndexOf("/") + 1);
}

export function parseFileListStatus(outputs: JobOutputRecord[]): FileListStatus | null {
  return parseLatestStatus(outputs, "file_list_dir") as FileListStatus | null;
}

export function parseFileReadTextStatus(outputs: JobOutputRecord[]): FileReadTextStatus | null {
  return parseLatestStatus(outputs, "file_read_text") as FileReadTextStatus | null;
}

export function parseLatestFileStatus(outputs: JobOutputRecord[], type?: string): FileOperationStatus | null {
  return parseLatestStatus(outputs, type) as FileOperationStatus | null;
}

export function decodedText(status: FileReadTextStatus): string {
  return new TextDecoder("utf-8", { fatal: false }).decode(base64ToBytes(status.content_base64));
}

export async function buildWriteTextOperation({
  content,
  create,
  expectedSha256Hex,
  mode,
  path,
  policy = "fail",
}: {
  content: string;
  create: boolean;
  expectedSha256Hex?: string | null;
  mode: string;
  path: string;
  policy?: FileActionPolicy;
}): Promise<JobOperation> {
  const bytes = new TextEncoder().encode(content);
  if (bytes.byteLength > FILE_BROWSER_TEXT_LIMIT_BYTES) {
    throw new Error("Editor content exceeds 1 MiB text limit");
  }
  return {
    type: "file_write_text",
    path: normalizeAbsolutePath(path),
    mode: parseFileMode(mode),
    size_bytes: bytes.byteLength,
    sha256_hex: await sha256Hex(bytes),
    content_base64: bytesToBase64(bytes),
    ...(expectedSha256Hex ? { expected_sha256_hex: expectedSha256Hex } : {}),
    ...(create ? { create: true } : {}),
    policy,
  };
}

export async function buildUploadOperation(
  file: File,
  destinationPath: string,
  mode: string,
  options: {
    existingPolicy?: FileExistingPolicy;
    owner?: string | null;
    group?: string | null;
    uid?: number | null;
    gid?: number | null;
    ownershipPolicy?: FileOwnershipPolicy;
  } = {},
): Promise<JobOperation> {
  const payload = await readFilePushPayload(file);
  const common = {
    path: normalizeAbsolutePath(destinationPath),
    mode: parseFileMode(mode),
    size_bytes: payload.sizeBytes,
    sha256_hex: payload.sha256Hex,
    existing_policy: options.existingPolicy ?? "skip",
    ...(options.owner ? { owner: options.owner } : {}),
    ...(options.group ? { group: options.group } : {}),
    ...(typeof options.uid === "number" ? { uid: options.uid } : {}),
    ...(typeof options.gid === "number" ? { gid: options.gid } : {}),
    ownership_policy: options.ownershipPolicy ?? "fail",
  };
  if (payload.chunks) {
    return { type: "file_push_chunked", ...common, chunks: payload.chunks };
  }
  return { type: "file_push", ...common, data_base64: payload.dataBase64 };
}

export function fileBrowserOperationLabel(operation: JobOperation): string {
  switch (operation.type) {
    case "file_list_dir":
      return `List ${operation.path}`;
    case "file_read_text":
      return `Open ${operation.path}`;
    case "file_write_text":
      return operation.create ? `Write text ${operation.path}` : `Save ${operation.path}`;
    case "file_mkdir":
      return `Create folder ${operation.path}`;
    case "file_rename":
      return `Rename ${operation.path}`;
    case "file_delete":
      return `Delete ${operation.path}`;
    case "file_chmod":
      return `Change mode ${operation.path}`;
    case "file_chown":
      return `Change owner ${operation.path}`;
    case "file_copy":
      return `Copy ${operation.path}`;
    case "file_download":
      return `Download ${operation.path}`;
    case "file_archive_tar":
      return `Archive ${operation.path}`;
    case "file_push":
    case "file_push_chunked":
      return `Upload ${operation.path}`;
    default:
      return operation.type;
  }
}

export function mutatesFileSystem(operation: JobOperation): boolean {
  return [
    "file_write_text",
    "file_mkdir",
    "file_rename",
    "file_delete",
    "file_chmod",
    "file_chown",
    "file_copy",
    "file_push",
    "file_push_chunked",
  ].includes(operation.type);
}

function parseLatestStatus(outputs: JobOutputRecord[], type?: string): JsonValue | null {
  for (const output of [...outputs].reverse()) {
    if (output.stream !== "status" || !output.data_base64) {
      continue;
    }
    try {
      const parsed = JSON.parse(decodeOutputPreview(output.data_base64)) as JsonValue;
      if (!isJsonObject(parsed)) {
        continue;
      }
      if (type && parsed.type !== type) {
        continue;
      }
      return parsed;
    } catch {
      continue;
    }
  }
  return null;
}
