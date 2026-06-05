import { useMemo, useState, type FormEvent } from "react";
import { RefreshCw } from "lucide-react";
import { buildRestoreRollbackOperation } from "../backups/restoreRollback";
import { ProofVaultBox } from "../components/ProofVaultBox";
import { bytesToBase64 } from "../fileTransfer";
import { usePanelDisplaySettings } from "../panelDisplay";
import { buildEnvelopesForOperation, parseCommandArgv, type ProofMaterial } from "../proof";
import { DEFAULT_BACKUP_SELECTED_PATHS, DEFAULT_RESTORE_SELECTED_PATHS } from "../presets/backupPathPresets";
import { ArtifactUploadForm } from "./backups/ArtifactUploadForm";
import { BackupHistoryTables } from "./backups/BackupHistoryTables";
import { BackupPolicyForm } from "./backups/BackupPolicyForm";
import { BackupPolicyPruneForm } from "./backups/BackupPolicyPruneForm";
import { BackupRequestForm } from "./backups/BackupRequestForm";
import { MigrationLinkForm } from "./backups/MigrationLinkForm";
import { RestorePlanForm } from "./backups/RestorePlanForm";
import { RestoreRollbackForm } from "./backups/RestoreRollbackForm";
import { RestoreRunForm } from "./backups/RestoreRunForm";
import type {
  AgentView,
  BackupArtifactRecord,
  BackupArtifactHandoffRecord,
  BackupArtifactHandoffRequest,
  BackupPolicyPruneRequest,
  BackupPolicyPruneResponse,
  BackupPolicyRecord,
  BackupRequestRecord,
  CreateBackupPolicyRequest,
  CreateBackupRequest,
  CreateJobRequest,
  CreateJobResponse,
  CreateMigrationLinkRequest,
  CreateRestorePlanRequest,
  JobOperation,
  JobOutputRecord,
  MigrationLinkRecord,
  PreparedBackupArtifactRestoreRecord,
  RestorePlanRecord,
  UploadBackupArtifactRequest,
} from "../types";
import {
  clientDisplayNameFromMap,
  clientDisplayNameMap,
  formatVpsName,
  runPanelAction,
  shortId,
} from "../utils";

type BackupsPanelProps = {
  agents: AgentView[];
  artifacts: BackupArtifactRecord[];
  backupPolicies: BackupPolicyRecord[];
  backups: BackupRequestRecord[];
  migrationLinks: MigrationLinkRecord[];
  restorePlans: RestorePlanRecord[];
  error: string | null;
  loading: boolean;
  onCreateBackupPolicy: (request: CreateBackupPolicyRequest) => Promise<BackupPolicyRecord>;
  onCreateBackupRequest: (request: CreateBackupRequest) => Promise<BackupRequestRecord>;
  onCreateJob: (request: CreateJobRequest) => Promise<CreateJobResponse>;
  onCreateMigrationLink: (request: CreateMigrationLinkRequest) => Promise<MigrationLinkRecord>;
  onCreateRestorePlan: (request: CreateRestorePlanRequest) => Promise<RestorePlanRecord>;
  onDownloadBackupArtifact: (backupRequestId: string) => Promise<Blob>;
  onHandoffBackupArtifact: (backupRequestId: string, request: BackupArtifactHandoffRequest) => Promise<BackupArtifactHandoffRecord>;
  onLoadJobOutputs: (jobId: string) => Promise<JobOutputRecord[]>;
  onPrepareBackupArtifactRestore: (
    backupRequestId: string,
    request: { private_key_hex: string; artifact_base64?: string | null },
  ) => Promise<PreparedBackupArtifactRestoreRecord>;
  onPruneBackupPolicies: (request: BackupPolicyPruneRequest) => Promise<BackupPolicyPruneResponse>;
  onUploadBackupArtifact: (backupRequestId: string, request: UploadBackupArtifactRequest) => Promise<BackupArtifactRecord>;
  onUploadBackupArtifactChunked: (
    backupRequestId: string,
    objectKey: string,
    artifactFile: File,
    confirmed: boolean,
  ) => Promise<BackupArtifactRecord>;
  onRefresh: () => Promise<void>;
};

type RestoreRunInput = {
  sourceBackupRequestId: string;
  targetClientId: string;
  paths: string[];
  includeConfig: boolean;
  destinationRoot: string;
  archivePath: string;
  archiveSha256Hex: string;
  artifactFile: File | null;
  dryRun: boolean;
  privateKeyHex: string;
  postRestoreArgv: string;
  timeoutSecs: number;
  forceUnprivileged: boolean;
};

type RestoreRunResult = {
  nextJob: CreateJobResponse;
  payloadHashHex: string;
};

const INLINE_BACKUP_ARTIFACT_UPLOAD_LIMIT_BYTES = 16 * 1024 * 1024;

export function BackupsPanel({
  agents,
  artifacts,
  backupPolicies,
  backups,
  migrationLinks,
  restorePlans,
  error,
  loading,
  onCreateBackupPolicy,
  onCreateBackupRequest,
  onCreateJob,
  onCreateMigrationLink,
  onCreateRestorePlan,
  onDownloadBackupArtifact,
  onHandoffBackupArtifact,
  onLoadJobOutputs,
  onPrepareBackupArtifactRestore,
  onPruneBackupPolicies,
  onUploadBackupArtifact,
  onUploadBackupArtifactChunked,
  onRefresh,
}: BackupsPanelProps) {
  const { vpsNameDisplayMode } = usePanelDisplaySettings();
  const [clientId, setClientId] = useState("");
  const [pathsText, setPathsText] = useState(DEFAULT_BACKUP_SELECTED_PATHS);
  const [includeConfig, setIncludeConfig] = useState(true);
  const [note, setNote] = useState("");
  const [confirmed, setConfirmed] = useState(false);
  const [proofTtlSecs, setProofTtlSecs] = useState(300);
  const [proofMaterial, setProofMaterial] = useState<ProofMaterial | null>(null);
  const [lastPayloadHash, setLastPayloadHash] = useState<string | null>(null);
  const [policyName, setPolicyName] = useState("nightly-backup");
  const [policyTargetsText, setPolicyTargetsText] = useState("tag:backup-critical");
  const [policyPathsText, setPolicyPathsText] = useState(DEFAULT_BACKUP_SELECTED_PATHS);
  const [policyIncludeConfig, setPolicyIncludeConfig] = useState(true);
  const [policyRecipientPublicKeyHex, setPolicyRecipientPublicKeyHex] = useState("");
  const [policyIntervalSecs, setPolicyIntervalSecs] = useState(86_400);
  const [policyRetentionDays, setPolicyRetentionDays] = useState(30);
  const [policyKeepLast, setPolicyKeepLast] = useState(7);
  const [policyRotationGeneration, setPolicyRotationGeneration] = useState("");
  const [policyEnabled, setPolicyEnabled] = useState(true);
  const [policyConfirmed, setPolicyConfirmed] = useState(false);
  const [policyPruneScheduleId, setPolicyPruneScheduleId] = useState("");
  const [policyPruneDryRun, setPolicyPruneDryRun] = useState(true);
  const [policyPruneMetadataOnly, setPolicyPruneMetadataOnly] = useState(false);
  const [policyPruneConfirmed, setPolicyPruneConfirmed] = useState(false);
  const [lastPolicy, setLastPolicy] = useState<BackupPolicyRecord | null>(null);
  const [lastPolicyPrune, setLastPolicyPrune] = useState<BackupPolicyPruneResponse | null>(null);
  const [lastRequest, setLastRequest] = useState<BackupRequestRecord | null>(null);
  const [artifactBackupId, setArtifactBackupId] = useState("");
  const [artifactObjectKey, setArtifactObjectKey] = useState("");
  const [artifactFile, setArtifactFile] = useState<File | null>(null);
  const [artifactConfirmed, setArtifactConfirmed] = useState(false);
  const [artifactUploadMode, setArtifactUploadMode] = useState<"inline" | "chunked">("inline");
  const [handoffJobId, setHandoffJobId] = useState("");
  const [handoffConfirmed, setHandoffConfirmed] = useState(false);
  const [lastArtifact, setLastArtifact] = useState<BackupArtifactRecord | null>(null);
  const [restoreSourceId, setRestoreSourceId] = useState("");
  const [restoreTargetId, setRestoreTargetId] = useState("");
  const [restorePathsText, setRestorePathsText] = useState(DEFAULT_RESTORE_SELECTED_PATHS);
  const [restoreIncludeConfig, setRestoreIncludeConfig] = useState(false);
  const [restoreDestinationRoot, setRestoreDestinationRoot] = useState("");
  const [restoreNote, setRestoreNote] = useState("");
  const [restoreConfirmed, setRestoreConfirmed] = useState(false);
  const [restoreArtifactFile, setRestoreArtifactFile] = useState<File | null>(null);
  const [restoreArchivePath, setRestoreArchivePath] = useState("");
  const [restoreArchiveSha256Hex, setRestoreArchiveSha256Hex] = useState("");
  const [restoreDryRun, setRestoreDryRun] = useState(false);
  const [restorePostRestoreArgv, setRestorePostRestoreArgv] = useState("");
  const [restorePrivateKeyHex, setRestorePrivateKeyHex] = useState("");
  const [restoreTimeoutSecs, setRestoreTimeoutSecs] = useState(60);
  const [restoreRunConfirmed, setRestoreRunConfirmed] = useState(false);
  const [restoreForceUnprivileged, setRestoreForceUnprivileged] = useState(false);
  const [rollbackRestoreJobId, setRollbackRestoreJobId] = useState("");
  const [rollbackTargetId, setRollbackTargetId] = useState("");
  const [rollbackTimeoutSecs, setRollbackTimeoutSecs] = useState(60);
  const [rollbackConfirmed, setRollbackConfirmed] = useState(false);
  const [rollbackForceUnprivileged, setRollbackForceUnprivileged] = useState(false);
  const [lastRestorePlan, setLastRestorePlan] = useState<RestorePlanRecord | null>(null);
  const [lastRestoreJob, setLastRestoreJob] = useState<CreateJobResponse | null>(null);
  const [lastRollbackJob, setLastRollbackJob] = useState<CreateJobResponse | null>(null);
  const [migrationRestorePlanId, setMigrationRestorePlanId] = useState("");
  const [migrationNote, setMigrationNote] = useState("");
  const [migrationConfirmed, setMigrationConfirmed] = useState(false);
  const [lastMigrationLink, setLastMigrationLink] = useState<MigrationLinkRecord | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [pending, setPending] = useState(false);
  const paths = useMemo(() => parseBackupPaths(pathsText), [pathsText]);
  const policyPaths = useMemo(() => parseBackupPaths(policyPathsText), [policyPathsText]);
  const policyTargets = useMemo(() => parseTargetSelectors(policyTargetsText), [policyTargetsText]);
  const restorePaths = useMemo(() => parseBackupPaths(restorePathsText), [restorePathsText]);
  const agentNameById = useMemo(() => clientDisplayNameMap(agents, vpsNameDisplayMode), [agents, vpsNameDisplayMode]);
  const selectedAgent = agents.find((agent) => agent.id === clientId) ?? null;
  const restoreTarget = agents.find((agent) => agent.id === restoreTargetId) ?? null;
  const rollbackTarget = agents.find((agent) => agent.id === rollbackTargetId) ?? null;
  const selectedMigrationRestorePlan = restorePlans.find((plan) => plan.id === migrationRestorePlanId) ?? null;
  const selectedMigrationSourceBackup = selectedMigrationRestorePlan
    ? backups.find((backup) => backup.id === selectedMigrationRestorePlan.source_backup_request_id) ?? null
    : null;
  const clientLabel = (clientId: string) => clientDisplayNameFromMap(clientId, agentNameById);
  const status =
    actionError ??
    (lastPolicyPrune
      ? policyPruneStatus(lastPolicyPrune)
      : lastPolicy
      ? `Policy ${lastPolicy.name} ${lastPolicy.enabled ? "enabled" : "disabled"}`
      : lastMigrationLink
      ? `Migration link ${shortId(lastMigrationLink.id)} ${lastMigrationLink.status}`
      : lastRollbackJob
      ? `Restore rollback job ${shortId(lastRollbackJob.job_id)} ${lastRollbackJob.status}`
      : lastRestoreJob
      ? `Restore job ${shortId(lastRestoreJob.job_id)} ${lastRestoreJob.status}`
      : lastArtifact
      ? `Artifact ${shortId(lastArtifact.id)} uploaded`
      : lastRestorePlan
      ? `Restore ${shortId(lastRestorePlan.id)} ${lastRestorePlan.status}`
      : lastRequest
      ? `Request ${shortId(lastRequest.id)} ${lastRequest.status}`
      : `${backupPolicies.length} polic${backupPolicies.length === 1 ? "y" : "ies"}, ${backups.length} backup request${
          backups.length === 1 ? "" : "s"
        }, ${artifacts.length} artifact${
          artifacts.length === 1 ? "" : "s"
        }, ${restorePlans.length} restore plan${restorePlans.length === 1 ? "" : "s"}, ${migrationLinks.length} migration link${
          migrationLinks.length === 1 ? "" : "s"
        }`);

  async function submitPolicy(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    await runPanelAction(setPending, setActionError, async () => {
      if (!policyName.trim()) {
        throw new Error("Policy name is required");
      }
      if (!policyIncludeConfig && policyPaths.length === 0) {
        throw new Error("Select config or at least one absolute path");
      }
      if (policyTargets.clients.length === 0 && policyTargets.tags.length === 0) {
        throw new Error("Add at least one client or tag selector");
      }
      const recipient = policyRecipientPublicKeyHex.trim().toLowerCase();
      if (recipient && !/^[0-9a-f]{64}$/.test(recipient)) {
        throw new Error("Recipient public key must be 32-byte hex");
      }
      if (!policyConfirmed) {
        throw new Error("Backup policy requires confirmation");
      }
      const policy = await onCreateBackupPolicy({
        name: policyName.trim(),
        clients: policyTargets.clients,
        tags: policyTargets.tags,
        paths: policyPaths,
        include_config: policyIncludeConfig,
        recipient_public_key_hex: recipient || null,
        retention_days: clampInteger(policyRetentionDays, 1, 3650),
        keep_last: clampInteger(policyKeepLast, 1, 1000),
        rotation_generation: policyRotationGeneration.trim() || null,
        interval_secs: clampInteger(policyIntervalSecs, 1, 31_536_000),
        start_at_unix: null,
        enabled: policyEnabled,
        catch_up_policy: "skip_missed",
        catch_up_limit: 1,
        retry_delay_secs: 300,
        max_failures: 3,
        confirmed: policyConfirmed,
      });
      setLastPolicy(policy);
      setPolicyConfirmed(false);
    });
  }

  async function submitPolicyPrune(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    await runPanelAction(setPending, setActionError, async () => {
      if (!policyPruneDryRun && !policyPruneConfirmed) {
        throw new Error("Policy prune requires confirmation");
      }
      const result = await onPruneBackupPolicies({
        schedule_id: policyPruneScheduleId || null,
        dry_run: policyPruneDryRun,
        metadata_only: policyPruneMetadataOnly,
        confirmed: policyPruneConfirmed,
      });
      setLastPolicyPrune(result);
      if (!policyPruneDryRun) {
        setPolicyPruneConfirmed(false);
      }
    });
  }

  async function submitRequest(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    await runPanelAction(setPending, setActionError, async () => {
      if (!proofMaterial) {
        throw new Error("Proof is locked");
      }
      if (!clientId) {
        throw new Error("Select a VPS");
      }
      if (!includeConfig && paths.length === 0) {
        throw new Error("Select config or at least one absolute path");
      }
      if (!confirmed) {
        throw new Error("Backup request requires confirmation");
      }
      const operation: JobOperation = { type: "backup", paths, include_config: includeConfig };
      const built = await buildEnvelopesForOperation({
        clientIds: [clientId],
        operation,
        proofTtlSecs,
        superPassword: proofMaterial.superPassword,
        superSaltHex: proofMaterial.superSaltHex,
      });
      const envelope = built.envelopes[clientId];
      if (!envelope) {
        throw new Error("Backup proof envelope was not generated");
      }
      const request = await onCreateBackupRequest({
        client_id: clientId,
        paths,
        include_config: includeConfig,
        confirmed,
        note: note.trim() || null,
        envelope,
      });
      setLastPayloadHash(built.payloadHashHex);
      setLastRequest(request);
      setArtifactBackupId(request.id);
      setArtifactObjectKey(`backups/${request.client_id}/${request.id}.json`);
    });
  }

  async function submitArtifactUpload(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    await runPanelAction(setPending, setActionError, async () => {
      if (!artifactBackupId) {
        throw new Error("Select a backup request");
      }
      if (!artifactObjectKey.trim()) {
        throw new Error("Object key is required");
      }
      if (!artifactFile) {
        throw new Error("Select an encrypted artifact file");
      }
      if (!artifactConfirmed) {
        throw new Error("Artifact upload requires confirmation");
      }
      const objectKey = artifactObjectKey.trim();
      const artifact =
        artifactUploadMode === "chunked"
          ? await onUploadBackupArtifactChunked(artifactBackupId, objectKey, artifactFile, artifactConfirmed)
          : await onUploadBackupArtifact(artifactBackupId, {
              object_key: objectKey,
              artifact_base64: await fileToBase64(artifactFile),
              confirmed: artifactConfirmed,
            });
      setLastArtifact(artifact);
    });
  }

  async function submitArtifactHandoff() {
    await runPanelAction(setPending, setActionError, async () => {
      if (!artifactBackupId) {
        throw new Error("Select a backup request");
      }
      if (!handoffConfirmed) {
        throw new Error("Retained output promotion requires confirmation");
      }
      const handoff = await onHandoffBackupArtifact(artifactBackupId, {
        confirmed: handoffConfirmed,
        job_id: handoffJobId.trim() || null,
      });
      setLastArtifact(handoff.artifact);
      setHandoffConfirmed(false);
    });
  }

  function selectArtifactBackupId(backupId: string) {
    setArtifactBackupId(backupId);
    const backup = backups.find((item) => item.id === backupId);
    if (backup) {
      setArtifactObjectKey(`backups/${backup.client_id}/${backup.id}.json`);
    }
  }

  function selectArtifactFile(file: File | null) {
    setArtifactFile(file);
    if (file && file.size > INLINE_BACKUP_ARTIFACT_UPLOAD_LIMIT_BYTES) {
      setArtifactUploadMode("chunked");
    }
  }

  async function submitRestorePlan(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    await runPanelAction(setPending, setActionError, async () => {
      if (!proofMaterial) {
        throw new Error("Proof is locked");
      }
      if (!restoreSourceId) {
        throw new Error("Select a source backup request");
      }
      if (!restoreTargetId) {
        throw new Error("Select a restore target");
      }
      if (!restoreIncludeConfig && restorePaths.length === 0) {
        throw new Error("Select config or at least one absolute path");
      }
      if (!restoreConfirmed) {
        throw new Error("Restore plan requires confirmation");
      }
      const operation: JobOperation = {
        type: "restore",
        source_backup_request_id: restoreSourceId,
        paths: restorePaths,
        include_config: restoreIncludeConfig,
        destination_root: restoreDestinationRoot.trim() || null,
        archive_base64: null,
        archive_size_bytes: null,
        archive_sha256_hex: null,
      };
      const built = await buildEnvelopesForOperation({
        clientIds: [restoreTargetId],
        operation,
        proofTtlSecs,
        superPassword: proofMaterial.superPassword,
        superSaltHex: proofMaterial.superSaltHex,
      });
      const envelope = built.envelopes[restoreTargetId];
      if (!envelope) {
        throw new Error("Restore proof envelope was not generated");
      }
      const plan = await onCreateRestorePlan({
        source_backup_request_id: restoreSourceId,
        target_client_id: restoreTargetId,
        paths: restorePaths,
        include_config: restoreIncludeConfig,
        destination_root: restoreDestinationRoot.trim() || null,
        confirmed: restoreConfirmed,
        note: restoreNote.trim() || null,
        envelope,
      });
      setLastPayloadHash(built.payloadHashHex);
      setLastRestorePlan(plan);
    });
  }

  async function dispatchRestoreRun(input: RestoreRunInput): Promise<RestoreRunResult> {
      if (!proofMaterial) {
        throw new Error("Proof is locked");
      }
      if (!input.sourceBackupRequestId) {
        throw new Error("Select a source backup request");
      }
      if (!input.targetClientId) {
        throw new Error("Select a restore target");
      }
      if (!input.includeConfig && input.paths.length === 0) {
        throw new Error("Select config or at least one absolute path");
      }
      if (input.includeConfig && !input.destinationRoot.trim()) {
        throw new Error("Config restore requires a destination root");
      }
      const archivePath = input.archivePath.trim();
      const archiveSha256Hex = input.archiveSha256Hex.trim().toLowerCase();
      if (archivePath && !archivePath.startsWith("/")) {
        throw new Error("Agent-local restore archive path must be absolute");
      }
      if (archiveSha256Hex && !/^[0-9a-f]{64}$/.test(archiveSha256Hex)) {
        throw new Error("Restore archive SHA-256 must be 64 hex characters");
      }
      if (!archivePath && !input.privateKeyHex.trim()) {
        throw new Error("Backup private key hex is required");
      }
      const sourceBackup = backups.find((backup) => backup.id === input.sourceBackupRequestId) ?? null;
      if (!archivePath && !input.artifactFile && sourceBackup && !sourceBackup.artifact_id) {
        throw new Error("Selected backup request has no stored artifact");
      }
      const postRestoreArgv = input.postRestoreArgv.trim() ? parseCommandArgv(input.postRestoreArgv) : [];
      let operation: JobOperation;
      if (archivePath) {
        operation = {
          type: "restore",
          source_backup_request_id: input.sourceBackupRequestId,
          paths: input.paths,
          include_config: input.includeConfig,
          destination_root: input.destinationRoot.trim() || null,
          archive_base64: null,
          archive_path: archivePath,
          archive_size_bytes: null,
          archive_sha256_hex: archiveSha256Hex || null,
          dry_run: input.dryRun,
          post_restore_argv: postRestoreArgv,
        };
      } else {
        const artifactBase64 = input.artifactFile
          ? bytesToBase64(new Uint8Array(await input.artifactFile.arrayBuffer()))
          : null;
        const artifact = await onPrepareBackupArtifactRestore(input.sourceBackupRequestId, {
          private_key_hex: input.privateKeyHex,
          artifact_base64: artifactBase64,
        });
        if (sourceBackup && artifact.artifact_client_id !== sourceBackup.client_id) {
          throw new Error("Artifact client does not match selected source backup");
        }
        operation = {
          type: "restore",
          source_backup_request_id: input.sourceBackupRequestId,
          paths: input.paths,
          include_config: input.includeConfig,
          destination_root: input.destinationRoot.trim() || null,
          archive_base64: artifact.archive_base64,
          archive_path: null,
          archive_size_bytes: artifact.archive_size_bytes,
          archive_sha256_hex: artifact.archive_sha256_hex,
          dry_run: input.dryRun,
          post_restore_argv: postRestoreArgv,
        };
      }
      const built = await buildEnvelopesForOperation({
        clientIds: [input.targetClientId],
        operation,
        proofTtlSecs,
        superPassword: proofMaterial.superPassword,
        superSaltHex: proofMaterial.superSaltHex,
      });
      const nextJob = await onCreateJob({
        clients: [input.targetClientId],
        tags: [],
        destructive: !input.dryRun,
        confirmed: true,
        command: "restore",
        argv: [],
        operation,
        timeout_secs: clampInteger(input.timeoutSecs, 1, 3600),
        force_unprivileged: input.forceUnprivileged,
        privileged: true,
        envelope: null,
        envelopes: built.envelopes,
      });
      return { nextJob, payloadHashHex: built.payloadHashHex };
  }

  async function submitRestoreRun() {
    await runPanelAction(setPending, setActionError, async () => {
      if (!restoreRunConfirmed) {
        throw new Error("Executable restore requires confirmation");
      }
      const { nextJob, payloadHashHex } = await dispatchRestoreRun({
        sourceBackupRequestId: restoreSourceId,
        targetClientId: restoreTargetId,
        paths: restorePaths,
        includeConfig: restoreIncludeConfig,
        destinationRoot: restoreDestinationRoot,
        archivePath: restoreArchivePath,
        archiveSha256Hex: restoreArchiveSha256Hex,
        artifactFile: restoreArtifactFile,
        dryRun: restoreDryRun,
        privateKeyHex: restorePrivateKeyHex,
        postRestoreArgv: restorePostRestoreArgv,
        timeoutSecs: restoreTimeoutSecs,
        forceUnprivileged: restoreForceUnprivileged,
      });
      setRestorePrivateKeyHex("");
      setLastPayloadHash(payloadHashHex);
      setLastRestoreJob(nextJob);
      setRollbackRestoreJobId(nextJob.job_id);
      setRollbackTargetId(restoreTargetId);
      setRollbackConfirmed(false);
    });
  }

  async function submitRestoreRollback() {
    await runPanelAction(setPending, setActionError, async () => {
      if (!proofMaterial) {
        throw new Error("Proof is locked");
      }
      if (!rollbackRestoreJobId.trim()) {
        throw new Error("Restore job ID is required");
      }
      if (!rollbackTargetId.trim()) {
        throw new Error("Target VPS is required");
      }
      if (!rollbackConfirmed) {
        throw new Error("Restore rollback requires confirmation");
      }
      const restoreJobId = rollbackRestoreJobId.trim();
      const targetClientId = rollbackTargetId.trim();
      const outputs = await onLoadJobOutputs(restoreJobId);
      const operation = buildRestoreRollbackOperation(restoreJobId, targetClientId, outputs);
      const built = await buildEnvelopesForOperation({
        clientIds: [targetClientId],
        operation,
        proofTtlSecs,
        superPassword: proofMaterial.superPassword,
        superSaltHex: proofMaterial.superSaltHex,
      });
      const nextJob = await onCreateJob({
        clients: [targetClientId],
        tags: [],
        destructive: true,
        confirmed: true,
        command: "restore_rollback",
        argv: [],
        operation,
        timeout_secs: clampInteger(rollbackTimeoutSecs, 1, 3600),
        force_unprivileged: rollbackForceUnprivileged,
        privileged: true,
        envelope: null,
        envelopes: built.envelopes,
      });
      setLastPayloadHash(built.payloadHashHex);
      setLastRollbackJob(nextJob);
      setRollbackConfirmed(false);
    });
  }

  async function submitMigrationLink() {
    await runPanelAction(setPending, setActionError, async () => {
      if (!migrationRestorePlanId) {
        throw new Error("Select a restore plan");
      }
      if (!migrationConfirmed) {
        throw new Error("Migration link requires confirmation");
      }
      const link = await onCreateMigrationLink({
        restore_plan_id: migrationRestorePlanId,
        confirmed: migrationConfirmed,
        note: migrationNote.trim() || null,
      });
      setLastMigrationLink(link);
      setMigrationConfirmed(false);
    });
  }

  async function submitMigrationRun() {
    await runPanelAction(setPending, setActionError, async () => {
      if (!selectedMigrationRestorePlan) {
        throw new Error("Select a restore plan");
      }
      if (!migrationConfirmed) {
        throw new Error("Migration run requires confirmation");
      }
      const link = await onCreateMigrationLink({
        restore_plan_id: selectedMigrationRestorePlan.id,
        confirmed: true,
        note: migrationNote.trim() || null,
      });
      const { nextJob, payloadHashHex } = await dispatchRestoreRun({
        sourceBackupRequestId: selectedMigrationRestorePlan.source_backup_request_id,
        targetClientId: selectedMigrationRestorePlan.target_client_id,
        paths: selectedMigrationRestorePlan.paths,
        includeConfig: selectedMigrationRestorePlan.include_config,
        destinationRoot: selectedMigrationRestorePlan.destination_root ?? "",
        archivePath: restoreArchivePath,
        archiveSha256Hex: restoreArchiveSha256Hex,
        artifactFile: restoreArtifactFile,
        dryRun: restoreDryRun,
        privateKeyHex: restorePrivateKeyHex,
        postRestoreArgv: restorePostRestoreArgv,
        timeoutSecs: restoreTimeoutSecs,
        forceUnprivileged: restoreForceUnprivileged,
      });
      setRestoreSourceId(selectedMigrationRestorePlan.source_backup_request_id);
      setRestoreTargetId(selectedMigrationRestorePlan.target_client_id);
      setRestorePathsText(selectedMigrationRestorePlan.paths.join("\n"));
      setRestoreIncludeConfig(selectedMigrationRestorePlan.include_config);
      setRestoreDestinationRoot(selectedMigrationRestorePlan.destination_root ?? "");
      setRestorePrivateKeyHex("");
      setLastMigrationLink(link);
      setLastPayloadHash(payloadHashHex);
      setLastRestoreJob(nextJob);
      setRollbackRestoreJobId(nextJob.job_id);
      setRollbackTargetId(selectedMigrationRestorePlan.target_client_id);
      setMigrationConfirmed(false);
      setRollbackConfirmed(false);
    });
  }

  return (
    <section className="workspace backupWorkspace">
      <div className="fleetPanel">
        <div className="sectionHeader">
          <div>
            <h2>Backup requests</h2>
            <span>{loading ? "Loading backup request history" : status}</span>
          </div>
          <button className="secondaryAction" onClick={() => void onRefresh()} type="button">
            <RefreshCw size={17} />
            Refresh
          </button>
        </div>
        <BackupHistoryTables
          artifacts={artifacts}
          backupPolicies={backupPolicies}
          backups={backups}
          clientLabel={clientLabel}
          error={error}
          migrationLinks={migrationLinks}
          restorePlans={restorePlans}
        />
      </div>

      <aside className="inspector backupInspector">
        <BackupPolicyForm
          includeConfig={policyIncludeConfig}
          intervalSecs={policyIntervalSecs}
          keepLast={policyKeepLast}
          name={policyName}
          onConfirmedChange={setPolicyConfirmed}
          onEnabledChange={setPolicyEnabled}
          onIncludeConfigChange={setPolicyIncludeConfig}
          onIntervalSecsChange={setPolicyIntervalSecs}
          onKeepLastChange={setPolicyKeepLast}
          onNameChange={setPolicyName}
          onPathsTextChange={setPolicyPathsText}
          onRecipientPublicKeyHexChange={setPolicyRecipientPublicKeyHex}
          onRetentionDaysChange={setPolicyRetentionDays}
          onRotationGenerationChange={setPolicyRotationGeneration}
          onSubmit={submitPolicy}
          onTargetsTextChange={setPolicyTargetsText}
          pathsCount={policyPaths.length}
          pathsText={policyPathsText}
          pending={pending}
          policyConfirmed={policyConfirmed}
          policyEnabled={policyEnabled}
          recipientPublicKeyHex={policyRecipientPublicKeyHex}
          retentionDays={policyRetentionDays}
          rotationGeneration={policyRotationGeneration}
          targetCount={policyTargets.clients.length + policyTargets.tags.length}
          targetsText={policyTargetsText}
        />
        <BackupPolicyPruneForm
          confirmed={policyPruneConfirmed}
          dryRun={policyPruneDryRun}
          metadataOnly={policyPruneMetadataOnly}
          onConfirmedChange={setPolicyPruneConfirmed}
          onDryRunChange={setPolicyPruneDryRun}
          onMetadataOnlyChange={setPolicyPruneMetadataOnly}
          onScheduleIdChange={setPolicyPruneScheduleId}
          onSubmit={submitPolicyPrune}
          pending={pending}
          policies={backupPolicies}
          result={lastPolicyPrune}
          scheduleId={policyPruneScheduleId}
        />
        <BackupRequestForm
          agents={agents}
          clientId={clientId}
          confirmed={confirmed}
          includeConfig={includeConfig}
          note={note}
          onClientIdChange={setClientId}
          onConfirmedChange={setConfirmed}
          onIncludeConfigChange={setIncludeConfig}
          onNoteChange={setNote}
          onPathsTextChange={setPathsText}
          onProofTtlSecsChange={setProofTtlSecs}
          onSubmit={submitRequest}
          pathsCount={paths.length}
          pathsText={pathsText}
          pending={pending}
          proofReady={Boolean(proofMaterial)}
          proofTtlSecs={proofTtlSecs}
          selectedAgentName={selectedAgent ? formatVpsName(selectedAgent, vpsNameDisplayMode) : null}
        />
        <ArtifactUploadForm
          artifactBackupId={artifactBackupId}
          artifactConfirmed={artifactConfirmed}
          artifactFile={artifactFile}
          artifactObjectKey={artifactObjectKey}
          artifactUploadMode={artifactUploadMode}
          backups={backups}
          clientLabel={clientLabel}
          handoffConfirmed={handoffConfirmed}
          handoffJobId={handoffJobId}
          onArtifactBackupIdChange={selectArtifactBackupId}
          onArtifactConfirmedChange={setArtifactConfirmed}
          onArtifactFileChange={selectArtifactFile}
          onArtifactObjectKeyChange={setArtifactObjectKey}
          onArtifactUploadModeChange={setArtifactUploadMode}
          onHandoffConfirmedChange={setHandoffConfirmed}
          onHandoffJobIdChange={setHandoffJobId}
          onHandoffSubmit={() => void submitArtifactHandoff()}
          onSubmit={submitArtifactUpload}
          pending={pending}
        />
        <RestorePlanForm
          agents={agents}
          backups={backups}
          onDestinationRootChange={setRestoreDestinationRoot}
          onIncludeConfigChange={setRestoreIncludeConfig}
          onNoteChange={setRestoreNote}
          onPathsTextChange={setRestorePathsText}
          onRestoreConfirmedChange={setRestoreConfirmed}
          onSourceIdChange={setRestoreSourceId}
          onSubmit={submitRestorePlan}
          onTargetIdChange={setRestoreTargetId}
          pending={pending}
          proofReady={Boolean(proofMaterial)}
          restoreConfirmed={restoreConfirmed}
          restoreDestinationRoot={restoreDestinationRoot}
          restoreIncludeConfig={restoreIncludeConfig}
          restoreNote={restoreNote}
          restorePathsCount={restorePaths.length}
          restorePathsText={restorePathsText}
          restoreSourceId={restoreSourceId}
          restoreTargetId={restoreTargetId}
          restoreTargetName={restoreTarget ? formatVpsName(restoreTarget, vpsNameDisplayMode) : null}
          clientLabel={clientLabel}
        />
        <RestoreRunForm
          forceUnprivileged={restoreForceUnprivileged}
          onForceUnprivilegedChange={setRestoreForceUnprivileged}
          onArtifactFileChange={setRestoreArtifactFile}
          onArchivePathChange={setRestoreArchivePath}
          onArchiveSha256HexChange={setRestoreArchiveSha256Hex}
          onDryRunChange={setRestoreDryRun}
          onPrivateKeyHexChange={setRestorePrivateKeyHex}
          onPostRestoreArgvChange={setRestorePostRestoreArgv}
          onRestoreRunConfirmedChange={setRestoreRunConfirmed}
          onRestoreTimeoutSecsChange={setRestoreTimeoutSecs}
          onRunRestore={() => void submitRestoreRun()}
          pending={pending}
          proofReady={Boolean(proofMaterial)}
          restoreArchivePath={restoreArchivePath}
          restoreArchiveSha256Hex={restoreArchiveSha256Hex}
          restoreArtifactFile={restoreArtifactFile}
          restoreDryRun={restoreDryRun}
          restorePrivateKeyHex={restorePrivateKeyHex}
          restorePostRestoreArgv={restorePostRestoreArgv}
          restoreRunConfirmed={restoreRunConfirmed}
          restoreSourceId={restoreSourceId}
          restoreTarget={restoreTarget}
          restoreTargetId={restoreTargetId}
          restoreTimeoutSecs={restoreTimeoutSecs}
        />
        <RestoreRollbackForm
          forceUnprivileged={rollbackForceUnprivileged}
          onForceUnprivilegedChange={setRollbackForceUnprivileged}
          onRestoreJobIdChange={setRollbackRestoreJobId}
          onRestoreRollbackConfirmedChange={setRollbackConfirmed}
          onRestoreRollbackTimeoutSecsChange={setRollbackTimeoutSecs}
          onRunRestoreRollback={() => void submitRestoreRollback()}
          onTargetClientIdChange={setRollbackTargetId}
          pending={pending}
          proofReady={Boolean(proofMaterial)}
          restoreJobId={rollbackRestoreJobId}
          restoreRollbackConfirmed={rollbackConfirmed}
          restoreRollbackTimeoutSecs={rollbackTimeoutSecs}
          targetAgent={rollbackTarget}
          targetClientId={rollbackTargetId}
        />
        <MigrationLinkForm
          archivePath={restoreArchivePath}
          clientLabel={clientLabel}
          forceUnprivileged={restoreForceUnprivileged}
          lastMigrationLink={lastMigrationLink}
          migrationConfirmed={migrationConfirmed}
          migrationNote={migrationNote}
          migrationRestorePlanId={migrationRestorePlanId}
          onMigrationConfirmedChange={setMigrationConfirmed}
          onMigrationNoteChange={setMigrationNote}
          onMigrationRestorePlanIdChange={setMigrationRestorePlanId}
          onRunMigrationRestore={() => void submitMigrationRun()}
          onSubmit={() => void submitMigrationLink()}
          pending={pending}
          postRestoreArgv={restorePostRestoreArgv}
          privateKeyReady={Boolean(restorePrivateKeyHex.trim())}
          proofReady={Boolean(proofMaterial)}
          restoreDryRun={restoreDryRun}
          restorePlans={restorePlans}
          selectedPlan={selectedMigrationRestorePlan}
          sourceBackup={selectedMigrationSourceBackup}
        />
        <ProofVaultBox lastPayloadHash={lastPayloadHash} onProofMaterialChange={setProofMaterial} proofMaterial={proofMaterial} />
      </aside>
    </section>
  );
}

function clampInteger(value: number, min: number, max: number): number {
  if (!Number.isFinite(value)) {
    return min;
  }
  return Math.trunc(Math.min(Math.max(value, min), max));
}

async function fileToBase64(file: File): Promise<string> {
  const bytes = new Uint8Array(await file.arrayBuffer());
  return bytesToBase64(bytes);
}

function parseBackupPaths(value: string): string[] {
  return Array.from(
    new Set(
      value
        .split(/[\n,]+/)
        .map((path) => path.trim())
        .filter((path) => path.length > 0 && path.startsWith("/")),
    ),
  );
}

function parseTargetSelectors(value: string): { clients: string[]; tags: string[] } {
  const result = { clients: [] as string[], tags: [] as string[] };
  for (const token of value
    .split(/[\s,]+/)
    .map((item) => item.trim())
    .filter(Boolean)) {
    if (token.startsWith("tag:")) {
      const target = token.slice("tag:".length).trim();
      if (!target) {
        continue;
      }
      result.tags.push(target);
    } else {
      result.tags.push(token);
    }
  }
  result.clients = Array.from(new Set(result.clients)).sort();
  result.tags = Array.from(new Set(result.tags)).sort();
  return result;
}

function policyPruneStatus(result: BackupPolicyPruneResponse): string {
  const totals = result.policies.reduce(
    (acc, policy) => ({
      matched: acc.matched + policy.matched_rows,
      pruned: acc.pruned + policy.pruned_rows,
    }),
    { matched: 0, pruned: 0 },
  );
  return `Policy prune ${result.dry_run ? "previewed" : "applied"} ${totals.pruned || totals.matched} artifact${
    totals.pruned === 1 || (!totals.pruned && totals.matched === 1) ? "" : "s"
  }`;
}
