import type { JobOperation, JobOutputRecord, RestoreRollbackFile } from "../types";
import { decodeOutputPreview } from "../utils";

type RestoreStatusFile = {
  archive_path?: string;
  destination_path?: string;
  rollback_path?: string | null;
  size_bytes?: number;
  sha256_hex?: string;
};

type RestoreStatus = {
  type?: string;
  rollback_available?: boolean;
  restored_files?: RestoreStatusFile[];
};

export function buildRestoreRollbackOperation(
  restoreJobId: string,
  targetClientId: string,
  outputs: JobOutputRecord[],
): JobOperation {
  const statusOutput = outputs.find(
    (output) =>
      output.client_id === targetClientId &&
      output.stream === "status" &&
      output.done &&
      output.exit_code === 0,
  );
  if (!statusOutput) {
    throw new Error("Restore status output was not found for the selected target");
  }
  const status = JSON.parse(decodeOutputPreview(statusOutput.data_base64)) as RestoreStatus;
  if (status.type !== "restore" || !status.rollback_available) {
    throw new Error("Selected job output is not rollback-capable restore status");
  }
  const restoredFiles = (status.restored_files ?? []).map(normalizeRestoredFile);
  if (restoredFiles.length === 0) {
    throw new Error("Restore status has no restored files");
  }
  return {
    type: "restore_rollback",
    source_restore_job_id: restoreJobId,
    restored_files: restoredFiles,
  };
}

function normalizeRestoredFile(file: RestoreStatusFile): RestoreRollbackFile {
  if (
    typeof file.archive_path !== "string" ||
    typeof file.destination_path !== "string" ||
    typeof file.size_bytes !== "number" ||
    typeof file.sha256_hex !== "string"
  ) {
    throw new Error("Restore status has an invalid restored file entry");
  }
  return {
    archive_path: file.archive_path,
    destination_path: file.destination_path,
    rollback_path: file.rollback_path ?? null,
    restored_size_bytes: file.size_bytes,
    restored_sha256_hex: file.sha256_hex,
  };
}
