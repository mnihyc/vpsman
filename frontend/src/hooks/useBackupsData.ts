import { useCallback, useRef, useState } from "react";
import { apiGet, apiGetBlob, apiPost, apiPut, buildListPath, isApiUnauthorized } from "../api";
import { bytesToBase64, readFileSlice, sha256FileHex } from "../fileTransfer";
import { FLEET_DETAIL_LIMIT, HISTORY_DETAIL_LIMIT } from "../constants";
import type {
  BackupArtifactRecord,
  BackupArtifactHandoffRecord,
  BackupArtifactHandoffRequest,
  BackupArtifactUploadSessionRecord,
  BackupPolicyPruneRequest,
  BackupPolicyPruneResponse,
  BackupPolicyRecord,
  BackupRequestRecord,
  CreateBackupPolicyRequest,
  CreateBackupRequest,
  CreateMigrationLinkRequest,
  CreateMigrationRunRequest,
  CreateMigrationRunResponse,
  CreateRestorePlanRequest,
  MigrationLinkRecord,
  RestorePlanRecord,
  UpdateBackupPolicyRequest,
  UploadBackupArtifactRequest,
} from "../types";

const BACKUP_ARTIFACT_UPLOAD_CHUNK_BYTES = 4 * 1024 * 1024;

function settledSourceFailure(
  label: string,
  result: PromiseSettledResult<unknown>,
): string | null {
  if (result.status === "fulfilled") {
    return null;
  }
  return `${label}: ${
    result.reason instanceof Error ? result.reason.message : "source unavailable"
  }`;
}

export function useBackupsData(
  apiToken: string,
  onUnauthorized: () => void,
  onAuditChanged: () => Promise<void>,
) {
  const apiTokenRef = useRef(apiToken);
  apiTokenRef.current = apiToken;
  const [backups, setBackups] = useState<BackupRequestRecord[]>([]);
  const [backupPolicies, setBackupPolicies] = useState<BackupPolicyRecord[]>([]);
  const [backupPoliciesTruncated, setBackupPoliciesTruncated] = useState(false);
  const [backupArtifacts, setBackupArtifacts] = useState<BackupArtifactRecord[]>([]);
  const [backupsTruncated, setBackupsTruncated] = useState(false);
  const [backupArtifactsTruncated, setBackupArtifactsTruncated] = useState(false);
  const [restorePlans, setRestorePlans] = useState<RestorePlanRecord[]>([]);
  const [migrationLinks, setMigrationLinks] = useState<MigrationLinkRecord[]>([]);
  const [backupsError, setBackupsError] = useState<string | null>(null);
  const [backupsLoading, setBackupsLoading] = useState(false);
  const [backupsEvidenceAvailable, setBackupsEvidenceAvailable] =
    useState(false);
  const backupsLoadGeneration = useRef(0);

  const loadBackups = useCallback(async () => {
    if (apiTokenRef.current !== apiToken) {
      return;
    }
    const generation = backupsLoadGeneration.current + 1;
    backupsLoadGeneration.current = generation;
    setBackupsLoading(true);
    setBackupsError(null);
    try {
      const results = await Promise.allSettled([
        apiGet<BackupRequestRecord[]>(
          buildListPath("/api/v1/backups", { limit: HISTORY_DETAIL_LIMIT, sort: "created_at", dir: "desc" }),
          apiToken,
        ),
        apiGet<BackupPolicyRecord[]>(
          `/api/v1/backup-policies?limit=${FLEET_DETAIL_LIMIT}`,
          apiToken,
        ),
        apiGet<BackupArtifactRecord[]>(
          buildListPath("/api/v1/backup-artifacts", { limit: HISTORY_DETAIL_LIMIT, sort: "created_at", dir: "desc" }),
          apiToken,
        ),
        apiGet<RestorePlanRecord[]>(
          buildListPath("/api/v1/restore-plans", { limit: HISTORY_DETAIL_LIMIT, sort: "created_at", dir: "desc" }),
          apiToken,
        ),
        apiGet<MigrationLinkRecord[]>(
          buildListPath("/api/v1/migration-links", { limit: HISTORY_DETAIL_LIMIT, sort: "created_at", dir: "desc" }),
          apiToken,
        ),
      ]);
      if (
        apiTokenRef.current !== apiToken ||
        backupsLoadGeneration.current !== generation
      ) {
        return;
      }
      const unauthorized = results.some(
        (result) =>
          result.status === "rejected" && isApiUnauthorized(result.reason),
      );
      if (unauthorized) {
        onUnauthorized();
        setBackupsEvidenceAvailable(false);
        setBackups([]);
        setBackupsTruncated(false);
        setBackupPolicies([]);
        setBackupPoliciesTruncated(false);
        setBackupArtifacts([]);
        setBackupArtifactsTruncated(false);
        setRestorePlans([]);
        setMigrationLinks([]);
        setBackupsError("Operator login required");
        return;
      }
      const [
        backupResult,
        policyResult,
        artifactResult,
        restoreResult,
        migrationResult,
      ] = results;
      setBackupsEvidenceAvailable(
        backupResult.status === "fulfilled" &&
          artifactResult.status === "fulfilled",
      );
      if (backupResult.status === "fulfilled") {
        setBackups(backupResult.value);
        setBackupsTruncated(
          backupResult.value.length >= HISTORY_DETAIL_LIMIT,
        );
      }
      if (policyResult.status === "fulfilled") {
        setBackupPolicies(policyResult.value);
        setBackupPoliciesTruncated(
          policyResult.value.length >= FLEET_DETAIL_LIMIT,
        );
      }
      if (artifactResult.status === "fulfilled") {
        setBackupArtifacts(artifactResult.value);
        setBackupArtifactsTruncated(
          artifactResult.value.length >= HISTORY_DETAIL_LIMIT,
        );
      }
      if (restoreResult.status === "fulfilled") {
        setRestorePlans(restoreResult.value);
      }
      if (migrationResult.status === "fulfilled") {
        setMigrationLinks(migrationResult.value);
      }
      const failures = [
        settledSourceFailure("Backup requests", backupResult),
        settledSourceFailure("Backup policies", policyResult),
        settledSourceFailure("Backup artifacts", artifactResult),
        settledSourceFailure("Restore plans", restoreResult),
        settledSourceFailure("Migration links", migrationResult),
      ]
        .filter((message): message is string => message !== null);
      setBackupsError(failures.length > 0 ? failures.join("; ") : null);
    } finally {
      if (backupsLoadGeneration.current === generation) {
        setBackupsLoading(false);
      }
    }
  }, [apiToken, onUnauthorized]);

  const createBackupRequest = useCallback(
    async (request: CreateBackupRequest) => {
      const response = await apiPost<BackupRequestRecord>("/api/v1/backups", apiToken, request);
      await Promise.all([loadBackups(), onAuditChanged()]);
      return response;
    },
    [apiToken, loadBackups, onAuditChanged],
  );

  const createBackupPolicy = useCallback(
    async (request: CreateBackupPolicyRequest) => {
      const response = await apiPost<BackupPolicyRecord>("/api/v1/backup-policies", apiToken, request);
      await Promise.all([loadBackups(), onAuditChanged()]);
      return response;
    },
    [apiToken, loadBackups, onAuditChanged],
  );

  const updateBackupPolicy = useCallback(
    async (scheduleId: string, request: UpdateBackupPolicyRequest) => {
      const response = await apiPut<BackupPolicyRecord>(
        `/api/v1/backup-policies/${encodeURIComponent(scheduleId)}`,
        apiToken,
        request,
      );
      await Promise.all([loadBackups(), onAuditChanged()]);
      return response;
    },
    [apiToken, loadBackups, onAuditChanged],
  );

  const pruneBackupPolicies = useCallback(
    async (request: BackupPolicyPruneRequest) => {
      const response = await apiPost<BackupPolicyPruneResponse>("/api/v1/backup-policies/prune", apiToken, request);
      await Promise.all([loadBackups(), onAuditChanged()]);
      return response;
    },
    [apiToken, loadBackups, onAuditChanged],
  );

  const createRestorePlan = useCallback(
    async (request: CreateRestorePlanRequest) => {
      const response = await apiPost<RestorePlanRecord>("/api/v1/restore-plans", apiToken, request);
      await Promise.all([loadBackups(), onAuditChanged()]);
      return response;
    },
    [apiToken, loadBackups, onAuditChanged],
  );

  const createMigrationLink = useCallback(
    async (request: CreateMigrationLinkRequest) => {
      const response = await apiPost<MigrationLinkRecord>("/api/v1/migration-links", apiToken, request);
      await Promise.all([loadBackups(), onAuditChanged()]);
      return response;
    },
    [apiToken, loadBackups, onAuditChanged],
  );

  const createMigrationRun = useCallback(
    async (request: CreateMigrationRunRequest) => {
      const response = await apiPost<CreateMigrationRunResponse>("/api/v1/migration-runs", apiToken, request);
      await Promise.all([loadBackups(), onAuditChanged()]);
      return response;
    },
    [apiToken, loadBackups, onAuditChanged],
  );

  const uploadBackupArtifact = useCallback(
    async (backupRequestId: string, request: UploadBackupArtifactRequest) => {
      const response = await apiPost<BackupArtifactRecord>(
        `/api/v1/backups/${backupRequestId}/artifact`,
        apiToken,
        request,
      );
      await Promise.all([loadBackups(), onAuditChanged()]);
      return response;
    },
    [apiToken, loadBackups, onAuditChanged],
  );

  const uploadBackupArtifactChunked = useCallback(
    async (
      backupRequestId: string,
      objectKey: string,
      artifactFile: File,
      confirmed: boolean,
      chunkSizeBytes = BACKUP_ARTIFACT_UPLOAD_CHUNK_BYTES,
    ) => {
      if (!confirmed) {
        throw new Error("Chunked artifact upload requires confirmation");
      }
      if (artifactFile.size <= 0) {
        throw new Error("Artifact file must not be empty");
      }
      const expectedSha256Hex = await sha256FileHex(artifactFile);
      let session: BackupArtifactUploadSessionRecord | null = null;
      try {
        session = await apiPost<BackupArtifactUploadSessionRecord>(
          `/api/v1/backups/${backupRequestId}/artifact-upload-sessions`,
          apiToken,
          {
            object_key: objectKey,
            expected_sha256_hex: expectedSha256Hex,
            expected_size_bytes: artifactFile.size,
            confirmed,
          },
        );
        const effectiveChunkSize = Math.max(1, Math.min(chunkSizeBytes, session.max_chunk_bytes));
        let offset = session.next_offset_bytes;
        while (offset < artifactFile.size) {
          const end = Math.min(offset + effectiveChunkSize, artifactFile.size);
          const chunk = await readFileSlice(artifactFile, offset, end);
          const view = await apiPost<BackupArtifactUploadSessionRecord>(
            `/api/v1/backups/${backupRequestId}/artifact-upload-sessions/${session.upload_id}/chunks`,
            apiToken,
            {
              offset_bytes: offset,
              data_base64: bytesToBase64(chunk),
            },
          );
          offset = view.next_offset_bytes;
        }
        const response = await apiPost<BackupArtifactRecord>(
          `/api/v1/backups/${backupRequestId}/artifact-upload-sessions/${session.upload_id}/commit`,
          apiToken,
          { confirmed },
        );
        await Promise.all([loadBackups(), onAuditChanged()]);
        return response;
      } catch (error) {
        let abortError: unknown = null;
        if (session) {
          try {
            await apiPost<BackupArtifactUploadSessionRecord>(
              `/api/v1/backups/${backupRequestId}/artifact-upload-sessions/${session.upload_id}/abort`,
              apiToken,
              { confirmed: true },
            );
          } catch (cleanupError) {
            abortError = cleanupError;
          }
        }
        if (abortError) {
          const uploadMessage =
            error instanceof Error ? error.message : "Artifact upload failed";
          const abortMessage =
            abortError instanceof Error
              ? abortError.message
              : "upload-session cleanup failed";
          throw new Error(
            `${uploadMessage}; upload-session cleanup also failed: ${abortMessage}`,
          );
        }
        throw error;
      }
    },
    [apiToken, loadBackups, onAuditChanged],
  );

  const handoffBackupArtifact = useCallback(
    async (backupRequestId: string, request: BackupArtifactHandoffRequest) => {
      const response = await apiPost<BackupArtifactHandoffRecord>(
        `/api/v1/backups/${backupRequestId}/artifact-handoff`,
        apiToken,
        request,
      );
      await Promise.all([loadBackups(), onAuditChanged()]);
      return response;
    },
    [apiToken, loadBackups, onAuditChanged],
  );

  const downloadBackupArtifact = useCallback(
    async (backupRequestId: string) => {
      try {
        return await apiGetBlob(`/api/v1/backups/${backupRequestId}/artifact`, apiToken);
      } catch (error) {
        if (
          apiTokenRef.current === apiToken &&
          isApiUnauthorized(error)
        ) {
          onUnauthorized();
        }
        throw error;
      }
    },
    [apiToken, onUnauthorized],
  );

  const clearBackups = useCallback(() => {
    apiTokenRef.current = "";
    backupsLoadGeneration.current += 1;
    setBackups([]);
    setBackupsTruncated(false);
    setBackupPolicies([]);
    setBackupPoliciesTruncated(false);
    setBackupArtifacts([]);
    setBackupArtifactsTruncated(false);
    setRestorePlans([]);
    setMigrationLinks([]);
    setBackupsError(null);
    setBackupsLoading(false);
    setBackupsEvidenceAvailable(false);
  }, []);

  return {
    backups,
    backupsTruncated,
    backupPolicies,
    backupPoliciesTruncated,
    backupArtifacts,
    backupArtifactsTruncated,
    restorePlans,
    migrationLinks,
    backupsError,
    backupsEvidenceAvailable,
    backupsLoading,
    createBackupRequest,
    createBackupPolicy,
    updateBackupPolicy,
    createMigrationLink,
    createMigrationRun,
    createRestorePlan,
    clearBackups,
    downloadBackupArtifact,
    handoffBackupArtifact,
    pruneBackupPolicies,
    uploadBackupArtifact,
    uploadBackupArtifactChunked,
    loadBackups,
  };
}
