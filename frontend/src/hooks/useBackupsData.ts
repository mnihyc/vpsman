import { useCallback, useState } from "react";
import { apiGet, apiGetBlob, apiPost, buildListPath, isApiUnauthorized } from "../api";
import { bytesToBase64, readFileSlice, sha256FileHex } from "../fileTransfer";
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
  UploadBackupArtifactRequest,
} from "../types";

const BACKUP_ARTIFACT_UPLOAD_CHUNK_BYTES = 4 * 1024 * 1024;

export function useBackupsData(
  apiToken: string,
  onUnauthorized: () => void,
  onAuditChanged: () => Promise<void>,
) {
  const [backups, setBackups] = useState<BackupRequestRecord[]>([]);
  const [backupPolicies, setBackupPolicies] = useState<BackupPolicyRecord[]>([]);
  const [backupArtifacts, setBackupArtifacts] = useState<BackupArtifactRecord[]>([]);
  const [restorePlans, setRestorePlans] = useState<RestorePlanRecord[]>([]);
  const [migrationLinks, setMigrationLinks] = useState<MigrationLinkRecord[]>([]);
  const [backupsError, setBackupsError] = useState<string | null>(null);
  const [backupsLoading, setBackupsLoading] = useState(false);

  const loadBackups = useCallback(async () => {
    setBackupsLoading(true);
    setBackupsError(null);
    try {
      const [backupRows, policyRows, artifactRows, restoreRows, migrationRows] = await Promise.all([
        apiGet<BackupRequestRecord[]>(
          buildListPath("/api/v1/backups", { limit: 1000, sort: "created_at", dir: "desc" }),
          apiToken,
        ),
        apiGet<BackupPolicyRecord[]>("/api/v1/backup-policies", apiToken),
        apiGet<BackupArtifactRecord[]>(
          buildListPath("/api/v1/backup-artifacts", { limit: 1000, sort: "created_at", dir: "desc" }),
          apiToken,
        ),
        apiGet<RestorePlanRecord[]>(
          buildListPath("/api/v1/restore-plans", { limit: 1000, sort: "created_at", dir: "desc" }),
          apiToken,
        ),
        apiGet<MigrationLinkRecord[]>(
          buildListPath("/api/v1/migration-links", { limit: 1000, sort: "created_at", dir: "desc" }),
          apiToken,
        ),
      ]);
      setBackups(backupRows);
      setBackupPolicies(policyRows);
      setBackupArtifacts(artifactRows);
      setRestorePlans(restoreRows);
      setMigrationLinks(migrationRows);
    } catch (error) {
      if (isApiUnauthorized(error)) {
        onUnauthorized();
        setBackups([]);
        setBackupPolicies([]);
        setBackupArtifacts([]);
        setRestorePlans([]);
        setMigrationLinks([]);
        setBackupsError("Operator login required");
        return;
      }
      setBackupsError(error instanceof Error ? error.message : "Backup and restore history unavailable");
    } finally {
      setBackupsLoading(false);
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
      const session = await apiPost<BackupArtifactUploadSessionRecord>(
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
        if (isApiUnauthorized(error)) {
          onUnauthorized();
        }
        throw error;
      }
    },
    [apiToken, onUnauthorized],
  );

  return {
    backups,
    backupPolicies,
    backupArtifacts,
    restorePlans,
    migrationLinks,
    backupsError,
    backupsLoading,
    createBackupRequest,
    createBackupPolicy,
    createMigrationLink,
    createMigrationRun,
    createRestorePlan,
    downloadBackupArtifact,
    handoffBackupArtifact,
    pruneBackupPolicies,
    uploadBackupArtifact,
    uploadBackupArtifactChunked,
    loadBackups,
  };
}
