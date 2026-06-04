export type FileTransferSessionRecord = {
  session_id: string;
  client_id: string;
  direction: "upload" | "download";
  status: string;
  path: string;
  size_bytes: number | null;
  progress_bytes: number;
  progress_ratio: number | null;
  sha256_hex: string | null;
  chunk_size_bytes: number | null;
  last_chunk_size_bytes: number | null;
  last_chunk_sha256_hex: string | null;
  rate_limit_kbps: number | null;
  resumed: boolean | null;
  last_event: string;
  last_job_id: string;
  last_command_type: string;
  last_seq: number;
  observed_at: string;
  handoff_available: boolean;
  handoff_object_key: string | null;
  handoff_download_path: string | null;
};

export type FileTransferHandoffRecord = {
  client_id: string;
  session_id: string;
  object_key: string;
  sha256_hex: string;
  size_bytes: number;
  chunk_count: number;
  source: string;
  download_path: string;
};

export type FileTransferSourceArtifactRecord = {
  id: string;
  name: string;
  object_key: string;
  sha256_hex: string;
  size_bytes: number;
  created_by: string | null;
  created_at: string;
  download_path: string;
};

export type UploadFileTransferSourceArtifactRequest = {
  name?: string;
  source_base64: string;
  sha256_hex: string;
  size_bytes: number;
  confirmed: boolean;
};
