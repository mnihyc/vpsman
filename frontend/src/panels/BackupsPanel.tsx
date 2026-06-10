import { useMemo, useState, type FormEvent, type ReactNode } from "react";
import { RefreshCw } from "lucide-react";
import { buildRestoreRollbackOperation } from "../backups/restoreRollback";
import { ConfirmationPrompt } from "../components/ConfirmationPrompt";
import { ConsoleActionDrawer } from "../components/ConsoleLayout";
import { PrivilegeVaultBox } from "../components/PrivilegeVaultBox";
import { bytesToBase64 } from "../fileTransfer";
import { usePanelDisplaySettings } from "../panelDisplay";
import {
  buildPrivilegeForJobOperation,
  parseCommandArgv,
  type PrivilegeMaterial,
} from "../privilege";
import {
  agentsMatchingExpression,
  parseSearchExpression,
  selectorExpressionForClientIds,
} from "../searchExpression";
import {
  DEFAULT_BACKUP_SELECTED_PATHS,
  DEFAULT_RESTORE_SELECTED_PATHS,
} from "../presets/backupPathPresets";
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
  activeSubpage: string;
  agents: AgentView[];
  artifacts: BackupArtifactRecord[];
  backupPolicies: BackupPolicyRecord[];
  backups: BackupRequestRecord[];
  migrationLinks: MigrationLinkRecord[];
  restorePlans: RestorePlanRecord[];
  error: string | null;
  loading: boolean;
  onCreateBackupPolicy: (
    request: CreateBackupPolicyRequest,
  ) => Promise<BackupPolicyRecord>;
  onCreateBackupRequest: (
    request: CreateBackupRequest,
  ) => Promise<BackupRequestRecord>;
  onCreateJob: (request: CreateJobRequest) => Promise<CreateJobResponse>;
  onCreateMigrationLink: (
    request: CreateMigrationLinkRequest,
  ) => Promise<MigrationLinkRecord>;
  onCreateRestorePlan: (
    request: CreateRestorePlanRequest,
  ) => Promise<RestorePlanRecord>;
  onDownloadBackupArtifact: (backupRequestId: string) => Promise<Blob>;
  onHandoffBackupArtifact: (
    backupRequestId: string,
    request: BackupArtifactHandoffRequest,
  ) => Promise<BackupArtifactHandoffRecord>;
  onLoadJobOutputs: (jobId: string) => Promise<JobOutputRecord[]>;
  onPrepareBackupArtifactRestore: (
    backupRequestId: string,
    request: { private_key_hex: string; artifact_base64?: string | null },
  ) => Promise<PreparedBackupArtifactRestoreRecord>;
  onPruneBackupPolicies: (
    request: BackupPolicyPruneRequest,
  ) => Promise<BackupPolicyPruneResponse>;
  onUploadBackupArtifact: (
    backupRequestId: string,
    request: UploadBackupArtifactRequest,
  ) => Promise<BackupArtifactRecord>;
  onUploadBackupArtifactChunked: (
    backupRequestId: string,
    objectKey: string,
    artifactFile: File,
    confirmed: boolean,
  ) => Promise<BackupArtifactRecord>;
  onOpenPrivilegeUnlock: () => void;
  onRefresh: () => Promise<void>;
  privilegeMaterial: PrivilegeMaterial | null;
  setPrivilegeMaterial: (material: PrivilegeMaterial | null) => void;
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

type BackupConfirmationAction =
  | "policy"
  | "policy-prune"
  | "backup-request"
  | "artifact-upload"
  | "artifact-handoff"
  | "restore-plan"
  | "restore-run"
  | "restore-rollback"
  | "migration-link"
  | "migration-run";

const INLINE_BACKUP_ARTIFACT_UPLOAD_LIMIT_BYTES = 16 * 1024 * 1024;
const backupSubpageSummaries: Record<
  string,
  { loading: string; title: string }
> = {
  requests: { loading: "Loading backup requests", title: "Backup requests" },
  policies: { loading: "Loading backup policies", title: "Backup policies" },
  artifacts: { loading: "Loading backup artifacts", title: "Backup artifacts" },
  restore: { loading: "Loading restore plans", title: "Restore operations" },
  migration: { loading: "Loading migration links", title: "Migration links" },
};

export function BackupsPanel({
  activeSubpage,
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
  onOpenPrivilegeUnlock,
  onUploadBackupArtifact,
  onUploadBackupArtifactChunked,
  onRefresh,
  privilegeMaterial,
  setPrivilegeMaterial,
}: BackupsPanelProps) {
  const { vpsNameDisplayMode } = usePanelDisplaySettings();
  const [clientId, setClientId] = useState("");
  const [pathsText, setPathsText] = useState(DEFAULT_BACKUP_SELECTED_PATHS);
  const [includeConfig, setIncludeConfig] = useState(true);
  const [note, setNote] = useState("");
  const [lastPayloadHash, setLastPayloadHash] = useState<string | null>(null);
  const [policyName, setPolicyName] = useState("nightly-backup");
  const [policyTargetsText, setPolicyTargetsText] = useState(
    "tag:backup-critical",
  );
  const [policyPathsText, setPolicyPathsText] = useState(
    DEFAULT_BACKUP_SELECTED_PATHS,
  );
  const [policyIncludeConfig, setPolicyIncludeConfig] = useState(true);
  const [policyRecipientPublicKeyHex, setPolicyRecipientPublicKeyHex] =
    useState("");
  const [policyCronExpr, setPolicyCronExpr] = useState("0 3 * * *");
  const [policyRetentionDays, setPolicyRetentionDays] = useState(30);
  const [policyKeepLast, setPolicyKeepLast] = useState(7);
  const [policyRotationGeneration, setPolicyRotationGeneration] = useState("");
  const [policyEnabled, setPolicyEnabled] = useState(true);
  const [policyPruneScheduleId, setPolicyPruneScheduleId] = useState("");
  const [policyPruneDryRun, setPolicyPruneDryRun] = useState(true);
  const [policyPruneMetadataOnly, setPolicyPruneMetadataOnly] = useState(false);
  const [lastPolicy, setLastPolicy] = useState<BackupPolicyRecord | null>(null);
  const [lastPolicyPrune, setLastPolicyPrune] =
    useState<BackupPolicyPruneResponse | null>(null);
  const [lastRequest, setLastRequest] = useState<BackupRequestRecord | null>(
    null,
  );
  const [artifactBackupId, setArtifactBackupId] = useState("");
  const [artifactObjectKey, setArtifactObjectKey] = useState("");
  const [artifactFile, setArtifactFile] = useState<File | null>(null);
  const [artifactUploadMode, setArtifactUploadMode] = useState<
    "inline" | "chunked"
  >("inline");
  const [handoffJobId, setHandoffJobId] = useState("");
  const [lastArtifact, setLastArtifact] = useState<BackupArtifactRecord | null>(
    null,
  );
  const [restoreSourceId, setRestoreSourceId] = useState("");
  const [restoreTargetId, setRestoreTargetId] = useState("");
  const [restorePathsText, setRestorePathsText] = useState(
    DEFAULT_RESTORE_SELECTED_PATHS,
  );
  const [restoreIncludeConfig, setRestoreIncludeConfig] = useState(false);
  const [restoreDestinationRoot, setRestoreDestinationRoot] = useState("");
  const [restoreNote, setRestoreNote] = useState("");
  const [restoreArtifactFile, setRestoreArtifactFile] = useState<File | null>(
    null,
  );
  const [restoreArchivePath, setRestoreArchivePath] = useState("");
  const [restoreArchiveSha256Hex, setRestoreArchiveSha256Hex] = useState("");
  const [restoreDryRun, setRestoreDryRun] = useState(false);
  const [restorePostRestoreArgv, setRestorePostRestoreArgv] = useState("");
  const [restorePrivateKeyHex, setRestorePrivateKeyHex] = useState("");
  const [restoreTimeoutSecs, setRestoreTimeoutSecs] = useState(60);
  const [restoreForceUnprivileged, setRestoreForceUnprivileged] =
    useState(false);
  const [rollbackRestoreJobId, setRollbackRestoreJobId] = useState("");
  const [rollbackTargetId, setRollbackTargetId] = useState("");
  const [rollbackTimeoutSecs, setRollbackTimeoutSecs] = useState(60);
  const [rollbackForceUnprivileged, setRollbackForceUnprivileged] =
    useState(false);
  const [lastRestorePlan, setLastRestorePlan] =
    useState<RestorePlanRecord | null>(null);
  const [lastRestoreJob, setLastRestoreJob] =
    useState<CreateJobResponse | null>(null);
  const [lastRollbackJob, setLastRollbackJob] =
    useState<CreateJobResponse | null>(null);
  const [migrationRestorePlanId, setMigrationRestorePlanId] = useState("");
  const [migrationNote, setMigrationNote] = useState("");
  const [lastMigrationLink, setLastMigrationLink] =
    useState<MigrationLinkRecord | null>(null);
  const [pendingConfirmation, setPendingConfirmation] =
    useState<BackupConfirmationAction | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [pending, setPending] = useState(false);
  const [workflowOpen, setWorkflowOpen] = useState(false);
  const paths = useMemo(() => parseBackupPaths(pathsText), [pathsText]);
  const policyPaths = useMemo(
    () => parseBackupPaths(policyPathsText),
    [policyPathsText],
  );
  const policyTargetParse = useMemo(
    () => parseSearchExpression(policyTargetsText),
    [policyTargetsText],
  );
  const policyTargetIds = useMemo(
    () =>
      policyTargetParse.error
        ? []
        : agentsMatchingExpression(agents, policyTargetsText).map((agent) => agent.id),
    [agents, policyTargetParse.error, policyTargetsText],
  );
  const policyTargetCount = policyTargetIds.length;
  const restorePaths = useMemo(
    () => parseBackupPaths(restorePathsText),
    [restorePathsText],
  );
  const agentNameById = useMemo(
    () => clientDisplayNameMap(agents, vpsNameDisplayMode),
    [agents, vpsNameDisplayMode],
  );
  const selectedAgent = agents.find((agent) => agent.id === clientId) ?? null;
  const restoreTarget =
    agents.find((agent) => agent.id === restoreTargetId) ?? null;
  const rollbackTarget =
    agents.find((agent) => agent.id === rollbackTargetId) ?? null;
  const selectedMigrationRestorePlan =
    restorePlans.find((plan) => plan.id === migrationRestorePlanId) ?? null;
  const selectedMigrationSourceBackup = selectedMigrationRestorePlan
    ? (backups.find(
        (backup) =>
          backup.id === selectedMigrationRestorePlan.source_backup_request_id,
      ) ?? null)
    : null;
  const clientLabel = (clientId: string) =>
    clientDisplayNameFromMap(clientId, agentNameById);
  const backupSubpage = [
    "requests",
    "policies",
    "artifacts",
    "restore",
    "migration",
  ].includes(activeSubpage)
    ? activeSubpage
    : "requests";
  const backupSubpageMeta = backupSubpageSummaries[backupSubpage];
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

  function submitPolicy(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setPendingConfirmation("policy");
  }

  async function executePolicy() {
    await runPanelAction(setPending, setActionError, async () => {
      if (!policyName.trim()) {
        throw new Error("Policy name is required");
      }
      if (!policyIncludeConfig && policyPaths.length === 0) {
        throw new Error("Select config or at least one absolute path");
      }
      if (policyTargetParse.error) {
        throw new Error(
          `Invalid target expression: ${policyTargetParse.error}`,
        );
      }
      if (policyTargetCount === 0) {
        throw new Error("Add at least one matching target selector");
      }
      const recipient = policyRecipientPublicKeyHex.trim().toLowerCase();
      if (recipient && !/^[0-9a-f]{64}$/.test(recipient)) {
        throw new Error("Recipient public key must be 32-byte hex");
      }
      const policy = await onCreateBackupPolicy({
        name: policyName.trim(),
        selector_expression: policyTargetsText.trim(),
        target_client_ids: policyTargetIds,
        paths: policyPaths,
        include_config: policyIncludeConfig,
        recipient_public_key_hex: recipient || null,
        retention_days: clampInteger(policyRetentionDays, 1, 3650),
        keep_last: clampInteger(policyKeepLast, 1, 1000),
        rotation_generation: policyRotationGeneration.trim() || null,
        cron_expr: policyCronExpr.trim(),
        timezone: "UTC",
        enabled: policyEnabled,
        catch_up_policy: "skip_missed",
        catch_up_limit: 1,
        retry_delay_secs: 300,
        max_failures: 3,
        confirmed: true,
      });
      setLastPolicy(policy);
    });
  }

  function submitPolicyPrune(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (policyPruneDryRun) {
      void executePolicyPrune();
    } else {
      setPendingConfirmation("policy-prune");
    }
  }

  async function executePolicyPrune() {
    await runPanelAction(setPending, setActionError, async () => {
      const result = await onPruneBackupPolicies({
        schedule_id: policyPruneScheduleId || null,
        dry_run: policyPruneDryRun,
        metadata_only: policyPruneMetadataOnly,
        confirmed: true,
      });
      setLastPolicyPrune(result);
    });
  }

  function submitRequest(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setPendingConfirmation("backup-request");
  }

  async function executeRequest() {
    await runPanelAction(setPending, setActionError, async () => {
      if (!privilegeMaterial) {
        throw new Error("Privilege unlock is locked");
      }
      if (!clientId) {
        throw new Error("Select a VPS");
      }
      if (!includeConfig && paths.length === 0) {
        throw new Error("Select config or at least one absolute path");
      }
      const operation: JobOperation = {
        type: "backup",
        paths,
        include_config: includeConfig,
      };
      const selectorExpression = selectorExpressionForClientIds([clientId]);
      const built = await buildPrivilegeForJobOperation({
        clientIds: [clientId],
        commandType: "backup",
        operation,
        privilegeMaterial,
        selectorExpression,
        timeoutSecs: 30,
      });
      const request = await onCreateBackupRequest({
        client_id: clientId,
        paths,
        include_config: includeConfig,
        confirmed: true,
        note: note.trim() || null,
        privilege_assertion: built.privilegeAssertion,
      });
      setLastPayloadHash(built.payloadHashHex);
      setLastRequest(request);
      setArtifactBackupId(request.id);
      setArtifactObjectKey(`backups/${request.client_id}/${request.id}.json`);
    });
  }

  function submitArtifactUpload(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setPendingConfirmation("artifact-upload");
  }

  async function executeArtifactUpload() {
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
      const objectKey = artifactObjectKey.trim();
      const artifact =
        artifactUploadMode === "chunked"
          ? await onUploadBackupArtifactChunked(
              artifactBackupId,
              objectKey,
              artifactFile,
              true,
            )
          : await onUploadBackupArtifact(artifactBackupId, {
              object_key: objectKey,
              artifact_base64: await fileToBase64(artifactFile),
              confirmed: true,
            });
      setLastArtifact(artifact);
    });
  }

  function submitArtifactHandoff() {
    setPendingConfirmation("artifact-handoff");
  }

  async function executeArtifactHandoff() {
    await runPanelAction(setPending, setActionError, async () => {
      if (!artifactBackupId) {
        throw new Error("Select a backup request");
      }
      const handoff = await onHandoffBackupArtifact(artifactBackupId, {
        confirmed: true,
        job_id: handoffJobId.trim() || null,
      });
      setLastArtifact(handoff.artifact);
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

  function submitRestorePlan(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setPendingConfirmation("restore-plan");
  }

  async function executeRestorePlan() {
    await runPanelAction(setPending, setActionError, async () => {
      if (!privilegeMaterial) {
        throw new Error("Privilege unlock is locked");
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
      const selectorExpression = selectorExpressionForClientIds([
        restoreTargetId,
      ]);
      const built = await buildPrivilegeForJobOperation({
        clientIds: [restoreTargetId],
        commandType: "restore",
        operation,
        privilegeMaterial,
        selectorExpression,
        timeoutSecs: 30,
      });
      const plan = await onCreateRestorePlan({
        source_backup_request_id: restoreSourceId,
        target_client_id: restoreTargetId,
        paths: restorePaths,
        include_config: restoreIncludeConfig,
        destination_root: restoreDestinationRoot.trim() || null,
        confirmed: true,
        note: restoreNote.trim() || null,
        privilege_assertion: built.privilegeAssertion,
      });
      setLastPayloadHash(built.payloadHashHex);
      setLastRestorePlan(plan);
    });
  }

  async function dispatchRestoreRun(
    input: RestoreRunInput,
  ): Promise<RestoreRunResult> {
    if (!privilegeMaterial) {
      throw new Error("Privilege unlock is locked");
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
    const sourceBackup =
      backups.find((backup) => backup.id === input.sourceBackupRequestId) ??
      null;
    if (
      !archivePath &&
      !input.artifactFile &&
      sourceBackup &&
      !sourceBackup.artifact_id
    ) {
      throw new Error("Selected backup request has no stored artifact");
    }
    const postRestoreArgv = input.postRestoreArgv.trim()
      ? parseCommandArgv(input.postRestoreArgv)
      : [];
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
      const artifact = await onPrepareBackupArtifactRestore(
        input.sourceBackupRequestId,
        {
          private_key_hex: input.privateKeyHex,
          artifact_base64: artifactBase64,
        },
      );
      if (
        sourceBackup &&
        artifact.artifact_client_id !== sourceBackup.client_id
      ) {
        throw new Error(
          "Artifact client does not match selected source backup",
        );
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
    const selectorExpression = selectorExpressionForClientIds([
      input.targetClientId,
    ]);
    const boundedTimeoutSecs = clampInteger(input.timeoutSecs, 1, 3600);
    const built = await buildPrivilegeForJobOperation({
      clientIds: [input.targetClientId],
      commandType: "restore",
      forceUnprivileged: input.forceUnprivileged,
      operation,
      privilegeMaterial,
      selectorExpression,
      timeoutSecs: boundedTimeoutSecs,
    });
    const nextJob = await onCreateJob({
      selector_expression: selectorExpression,
      target_client_ids: [input.targetClientId],
      destructive: !input.dryRun,
      confirmed: true,
      command: "restore",
      argv: [],
      operation,
      timeout_secs: boundedTimeoutSecs,
      force_unprivileged: input.forceUnprivileged,
      privileged: true,
      privilege_assertion: built.privilegeAssertion,
    });
    return { nextJob, payloadHashHex: built.payloadHashHex };
  }

  function submitRestoreRun() {
    setPendingConfirmation("restore-run");
  }

  async function executeRestoreRun() {
    await runPanelAction(setPending, setActionError, async () => {
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
    });
  }

  function submitRestoreRollback() {
    setPendingConfirmation("restore-rollback");
  }

  async function executeRestoreRollback() {
    await runPanelAction(setPending, setActionError, async () => {
      if (!privilegeMaterial) {
        throw new Error("Privilege unlock is locked");
      }
      if (!rollbackRestoreJobId.trim()) {
        throw new Error("Restore job ID is required");
      }
      if (!rollbackTargetId.trim()) {
        throw new Error("Target VPS is required");
      }
      const restoreJobId = rollbackRestoreJobId.trim();
      const targetClientId = rollbackTargetId.trim();
      const outputs = await onLoadJobOutputs(restoreJobId);
      const operation = buildRestoreRollbackOperation(
        restoreJobId,
        targetClientId,
        outputs,
      );
      const selectorExpression = selectorExpressionForClientIds([
        targetClientId,
      ]);
      const boundedTimeoutSecs = clampInteger(rollbackTimeoutSecs, 1, 3600);
      const built = await buildPrivilegeForJobOperation({
        clientIds: [targetClientId],
        commandType: "restore_rollback",
        forceUnprivileged: rollbackForceUnprivileged,
        operation,
        privilegeMaterial,
        selectorExpression,
        timeoutSecs: boundedTimeoutSecs,
      });
      const nextJob = await onCreateJob({
        selector_expression: selectorExpression,
        target_client_ids: [targetClientId],
        destructive: true,
        confirmed: true,
        command: "restore_rollback",
        argv: [],
        operation,
        timeout_secs: boundedTimeoutSecs,
        force_unprivileged: rollbackForceUnprivileged,
        privileged: true,
        privilege_assertion: built.privilegeAssertion,
      });
      setLastPayloadHash(built.payloadHashHex);
      setLastRollbackJob(nextJob);
    });
  }

  function submitMigrationLink() {
    setPendingConfirmation("migration-link");
  }

  async function executeMigrationLink() {
    await runPanelAction(setPending, setActionError, async () => {
      if (!migrationRestorePlanId) {
        throw new Error("Select a restore plan");
      }
      const link = await onCreateMigrationLink({
        restore_plan_id: migrationRestorePlanId,
        confirmed: true,
        note: migrationNote.trim() || null,
      });
      setLastMigrationLink(link);
    });
  }

  function submitMigrationRun() {
    setPendingConfirmation("migration-run");
  }

  async function executeMigrationRun() {
    await runPanelAction(setPending, setActionError, async () => {
      if (!selectedMigrationRestorePlan) {
        throw new Error("Select a restore plan");
      }
      const link = await onCreateMigrationLink({
        restore_plan_id: selectedMigrationRestorePlan.id,
        confirmed: true,
        note: migrationNote.trim() || null,
      });
      const { nextJob, payloadHashHex } = await dispatchRestoreRun({
        sourceBackupRequestId:
          selectedMigrationRestorePlan.source_backup_request_id,
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
      setRestoreDestinationRoot(
        selectedMigrationRestorePlan.destination_root ?? "",
      );
      setRestorePrivateKeyHex("");
      setLastMigrationLink(link);
      setLastPayloadHash(payloadHashHex);
      setLastRestoreJob(nextJob);
      setRollbackRestoreJobId(nextJob.job_id);
      setRollbackTargetId(selectedMigrationRestorePlan.target_client_id);
    });
  }

  function buildBackupConfirmationItems(
    action: BackupConfirmationAction,
  ): Array<{ label: string; value: ReactNode }> {
    switch (action) {
      case "policy":
        return [
          { label: "Policy", value: policyName.trim() || "unnamed" },
          { label: "Fixed targets", value: `${policyTargetCount} VPSs resolved and saved` },
          {
            label: "Scope",
            value: `${policyIncludeConfig ? "config, " : ""}${policyPaths.length} paths`,
          },
          {
            label: "Schedule",
            value: policyCronExpr.trim() || "cron required",
          },
        ];
      case "policy-prune":
        return [
          {
            label: "Scope",
            value: policyPruneScheduleId
              ? shortId(policyPruneScheduleId)
              : "all policies",
          },
          {
            label: "Mode",
            value: policyPruneMetadataOnly
              ? "metadata only"
              : "metadata and objects",
          },
        ];
      case "backup-request":
        return [
          {
            label: "VPS",
            value: selectedAgent
              ? formatVpsName(selectedAgent, vpsNameDisplayMode)
              : clientId || "none",
          },
          {
            label: "Scope",
            value: `${includeConfig ? "config, " : ""}${paths.length} paths`,
          },
          {
            label: "Privilege",
            value: privilegeMaterial ? "Unlocked locally" : "Locked",
          },
        ];
      case "artifact-upload":
        return [
          {
            label: "Request",
            value: artifactBackupId ? shortId(artifactBackupId) : "none",
          },
          { label: "Object", value: artifactObjectKey.trim() || "missing" },
          { label: "Mode", value: artifactUploadMode },
        ];
      case "artifact-handoff":
        return [
          {
            label: "Request",
            value: artifactBackupId ? shortId(artifactBackupId) : "none",
          },
          {
            label: "Source job",
            value: handoffJobId.trim() || "latest retained output",
          },
        ];
      case "restore-plan":
        return [
          {
            label: "Source",
            value: restoreSourceId ? shortId(restoreSourceId) : "none",
          },
          {
            label: "Target",
            value: restoreTarget
              ? formatVpsName(restoreTarget, vpsNameDisplayMode)
              : restoreTargetId || "none",
          },
          {
            label: "Scope",
            value: `${restoreIncludeConfig ? "config, " : ""}${restorePaths.length} paths`,
          },
          {
            label: "Privilege",
            value: privilegeMaterial ? "Unlocked locally" : "Locked",
          },
        ];
      case "restore-run":
        return [
          {
            label: "Source",
            value: restoreSourceId ? shortId(restoreSourceId) : "none",
          },
          {
            label: "Target",
            value: restoreTarget
              ? formatVpsName(restoreTarget, vpsNameDisplayMode)
              : restoreTargetId || "none",
          },
          { label: "Mode", value: restoreDryRun ? "dry run" : "live restore" },
          {
            label: "Privilege",
            value: privilegeMaterial ? "Unlocked locally" : "Locked",
          },
        ];
      case "restore-rollback":
        return [
          {
            label: "Restore job",
            value: rollbackRestoreJobId.trim()
              ? shortId(rollbackRestoreJobId.trim())
              : "none",
          },
          {
            label: "Target",
            value: rollbackTarget
              ? formatVpsName(rollbackTarget, vpsNameDisplayMode)
              : rollbackTargetId || "none",
          },
          {
            label: "Privilege",
            value: privilegeMaterial ? "Unlocked locally" : "Locked",
          },
        ];
      case "migration-link":
        return [
          {
            label: "Restore plan",
            value: migrationRestorePlanId
              ? shortId(migrationRestorePlanId)
              : "none",
          },
          { label: "Note", value: migrationNote.trim() || "none" },
        ];
      case "migration-run":
        return [
          {
            label: "Restore plan",
            value: selectedMigrationRestorePlan
              ? shortId(selectedMigrationRestorePlan.id)
              : "none",
          },
          {
            label: "Route",
            value: selectedMigrationRestorePlan
              ? `${clientLabel(selectedMigrationRestorePlan.source_client_id)} to ${clientLabel(selectedMigrationRestorePlan.target_client_id)}`
              : "none",
          },
          { label: "Mode", value: restoreDryRun ? "dry run" : "live restore" },
          {
            label: "Privilege",
            value: privilegeMaterial ? "Unlocked locally" : "Locked",
          },
        ];
    }
  }

  function backupConfirmationDetail(
    action: BackupConfirmationAction | null,
  ): string {
    switch (action) {
      case "policy":
        return "Confirm the saved schedule, target snapshot, and backup scope.";
      case "policy-prune":
        return "Confirm pruning retained backup metadata and object references for the selected policy scope.";
      case "backup-request":
        return "Confirm this browser-unlocked backup request before it is written.";
      case "artifact-upload":
        return "Confirm the encrypted artifact upload for the selected backup request.";
      case "artifact-handoff":
        return "Confirm promoting retained job output into a backup artifact record.";
      case "restore-plan":
        return "Confirm the restore intent and target before saving the plan.";
      case "restore-run":
        return restoreDryRun
          ? "Confirm the restore rehearsal dispatch."
          : "Confirm the live restore dispatch.";
      case "restore-rollback":
        return "Confirm restore rollback dispatch for the selected target.";
      case "migration-link":
        return "Confirm writing the migration link for the selected restore plan.";
      case "migration-run":
        return restoreDryRun
          ? "Confirm migration link and restore rehearsal dispatch."
          : "Confirm migration link and live restore dispatch.";
      default:
        return "";
    }
  }

  async function confirmBackupAction() {
    const action = pendingConfirmation;
    if (!action) {
      return;
    }
    setPendingConfirmation(null);
    switch (action) {
      case "policy":
        await executePolicy();
        break;
      case "policy-prune":
        await executePolicyPrune();
        break;
      case "backup-request":
        await executeRequest();
        break;
      case "artifact-upload":
        await executeArtifactUpload();
        break;
      case "artifact-handoff":
        await executeArtifactHandoff();
        break;
      case "restore-plan":
        await executeRestorePlan();
        break;
      case "restore-run":
        await executeRestoreRun();
        break;
      case "restore-rollback":
        await executeRestoreRollback();
        break;
      case "migration-link":
        await executeMigrationLink();
        break;
      case "migration-run":
        await executeMigrationRun();
        break;
    }
  }

  const backupConfirmationTitle =
    pendingConfirmation === "policy"
      ? "Save backup policy"
      : pendingConfirmation === "policy-prune"
        ? "Prune backup artifacts"
        : pendingConfirmation === "backup-request"
          ? "Request backup"
          : pendingConfirmation === "artifact-upload"
            ? "Upload backup artifact"
            : pendingConfirmation === "artifact-handoff"
              ? "Promote retained output"
              : pendingConfirmation === "restore-plan"
                ? "Create restore plan"
                : pendingConfirmation === "restore-run"
                  ? "Run restore"
                  : pendingConfirmation === "restore-rollback"
                    ? "Rollback restore"
                    : pendingConfirmation === "migration-link"
                      ? "Link migration"
                      : "Run migration restore";
  const backupConfirmationItems = pendingConfirmation
    ? buildBackupConfirmationItems(pendingConfirmation)
    : [];
  const backupConfirmationTone =
    pendingConfirmation === "policy-prune" ||
    pendingConfirmation === "restore-run" ||
    pendingConfirmation === "restore-rollback" ||
    pendingConfirmation === "migration-run"
      ? "danger"
      : "normal";

  const backupWorkflowLabel =
    backupSubpage === "policies"
      ? "Open policy workflow"
      : backupSubpage === "artifacts"
        ? "Open artifact workflow"
        : backupSubpage === "restore"
          ? "Open restore workflow"
          : backupSubpage === "migration"
            ? "Open migration workflow"
            : "Open backup request";

  return (
    <section className="workspace singleColumn backupWorkspace backupSingleWorkspace">
      <div className="fleetPanel">
        <div className="sectionHeader">
          <div>
            <h2>{backupSubpageMeta.title}</h2>
            <span>{loading ? backupSubpageMeta.loading : status}</span>
          </div>
          <div className="sectionActions">
            <button
              className="secondaryAction"
              onClick={() => void onRefresh()}
              type="button"
            >
              <RefreshCw size={17} />
              Refresh
            </button>
            <button
              className="primaryAction"
              onClick={() => setWorkflowOpen(true)}
              type="button"
            >
              {backupWorkflowLabel}
            </button>
          </div>
        </div>
        <BackupHistoryTables
          activeSubpage={backupSubpage}
          artifacts={artifacts}
          backupPolicies={backupPolicies}
          backups={backups}
          clientLabel={clientLabel}
          error={error}
          migrationLinks={migrationLinks}
          restorePlans={restorePlans}
        />
      </div>
      <ConsoleActionDrawer
        description="One-time backup, restore, artifact, and migration inputs stay out of the data table until needed."
        onClose={() => setWorkflowOpen(false)}
        open={workflowOpen}
        title={backupWorkflowLabel}
      >
        <div className="backupInspector backupWorkflowBody">
          <ConfirmationPrompt
            confirmLabel="Confirm"
            detail={backupConfirmationDetail(pendingConfirmation)}
            items={backupConfirmationItems}
            onCancel={() => setPendingConfirmation(null)}
            onConfirm={() => void confirmBackupAction()}
            open={pendingConfirmation !== null}
            pending={pending}
            title={backupConfirmationTitle}
            tone={backupConfirmationTone}
          />
          {backupSubpage === "policies" && (
            <>
              <BackupPolicyForm
                agents={agents}
                confirmationOpen={pendingConfirmation === "policy"}
                cronExpr={policyCronExpr}
                includeConfig={policyIncludeConfig}
                keepLast={policyKeepLast}
                name={policyName}
                onCronExprChange={setPolicyCronExpr}
                onEnabledChange={setPolicyEnabled}
                onIncludeConfigChange={setPolicyIncludeConfig}
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
                policyEnabled={policyEnabled}
                recipientPublicKeyHex={policyRecipientPublicKeyHex}
                retentionDays={policyRetentionDays}
                rotationGeneration={policyRotationGeneration}
                targetCount={policyTargetCount}
                targetExpressionMessage={
                  policyTargetParse.error ??
                  `${policyTargetCount}/${agents.length}`
                }
                targetExpressionValid={!policyTargetParse.error}
                targetsText={policyTargetsText}
              />
              <BackupPolicyPruneForm
                confirmationOpen={pendingConfirmation === "policy-prune"}
                dryRun={policyPruneDryRun}
                metadataOnly={policyPruneMetadataOnly}
                onDryRunChange={setPolicyPruneDryRun}
                onMetadataOnlyChange={setPolicyPruneMetadataOnly}
                onScheduleIdChange={setPolicyPruneScheduleId}
                onSubmit={submitPolicyPrune}
                pending={pending}
                policies={backupPolicies}
                result={lastPolicyPrune}
                scheduleId={policyPruneScheduleId}
              />
            </>
          )}
          {backupSubpage === "requests" && (
            <BackupRequestForm
              agents={agents}
              clientId={clientId}
              confirmationOpen={pendingConfirmation === "backup-request"}
              includeConfig={includeConfig}
              note={note}
              onClientIdChange={setClientId}
              onIncludeConfigChange={setIncludeConfig}
              onNoteChange={setNote}
              onPathsTextChange={setPathsText}
              onSubmit={submitRequest}
              pathsCount={paths.length}
              pathsText={pathsText}
              pending={pending}
              privilegeReady={Boolean(privilegeMaterial)}
              selectedAgentName={
                selectedAgent
                  ? formatVpsName(selectedAgent, vpsNameDisplayMode)
                  : null
              }
            />
          )}
          {backupSubpage === "artifacts" && (
            <ArtifactUploadForm
              artifactBackupId={artifactBackupId}
              artifactConfirmationOpen={
                pendingConfirmation === "artifact-upload"
              }
              artifactFile={artifactFile}
              artifactObjectKey={artifactObjectKey}
              artifactUploadMode={artifactUploadMode}
              backups={backups}
              clientLabel={clientLabel}
              handoffConfirmationOpen={
                pendingConfirmation === "artifact-handoff"
              }
              handoffJobId={handoffJobId}
              onArtifactBackupIdChange={selectArtifactBackupId}
              onArtifactFileChange={selectArtifactFile}
              onArtifactObjectKeyChange={setArtifactObjectKey}
              onArtifactUploadModeChange={setArtifactUploadMode}
              onHandoffJobIdChange={setHandoffJobId}
              onHandoffSubmit={submitArtifactHandoff}
              onSubmit={submitArtifactUpload}
              pending={pending}
            />
          )}
          {backupSubpage === "restore" && (
            <>
              <RestorePlanForm
                agents={agents}
                backups={backups}
                confirmationOpen={pendingConfirmation === "restore-plan"}
                onDestinationRootChange={setRestoreDestinationRoot}
                onIncludeConfigChange={setRestoreIncludeConfig}
                onNoteChange={setRestoreNote}
                onPathsTextChange={setRestorePathsText}
                onSourceIdChange={setRestoreSourceId}
                onSubmit={submitRestorePlan}
                onTargetIdChange={setRestoreTargetId}
                pending={pending}
                privilegeReady={Boolean(privilegeMaterial)}
                restoreDestinationRoot={restoreDestinationRoot}
                restoreIncludeConfig={restoreIncludeConfig}
                restoreNote={restoreNote}
                restorePathsCount={restorePaths.length}
                restorePathsText={restorePathsText}
                restoreSourceId={restoreSourceId}
                restoreTargetId={restoreTargetId}
                restoreTargetName={
                  restoreTarget
                    ? formatVpsName(restoreTarget, vpsNameDisplayMode)
                    : null
                }
                clientLabel={clientLabel}
              />
              <RestoreRunForm
                confirmationOpen={pendingConfirmation === "restore-run"}
                forceUnprivileged={restoreForceUnprivileged}
                onForceUnprivilegedChange={setRestoreForceUnprivileged}
                onArtifactFileChange={setRestoreArtifactFile}
                onArchivePathChange={setRestoreArchivePath}
                onArchiveSha256HexChange={setRestoreArchiveSha256Hex}
                onDryRunChange={setRestoreDryRun}
                onPrivateKeyHexChange={setRestorePrivateKeyHex}
                onPostRestoreArgvChange={setRestorePostRestoreArgv}
                onRestoreTimeoutSecsChange={setRestoreTimeoutSecs}
                onRunRestore={submitRestoreRun}
                pending={pending}
                privilegeReady={Boolean(privilegeMaterial)}
                restoreArchivePath={restoreArchivePath}
                restoreArchiveSha256Hex={restoreArchiveSha256Hex}
                restoreArtifactFile={restoreArtifactFile}
                restoreDryRun={restoreDryRun}
                restorePrivateKeyHex={restorePrivateKeyHex}
                restorePostRestoreArgv={restorePostRestoreArgv}
                restoreSourceId={restoreSourceId}
                restoreTarget={restoreTarget}
                restoreTargetId={restoreTargetId}
                restoreTimeoutSecs={restoreTimeoutSecs}
              />
              <RestoreRollbackForm
                confirmationOpen={pendingConfirmation === "restore-rollback"}
                forceUnprivileged={rollbackForceUnprivileged}
                onForceUnprivilegedChange={setRollbackForceUnprivileged}
                onRestoreJobIdChange={setRollbackRestoreJobId}
                onRestoreRollbackTimeoutSecsChange={setRollbackTimeoutSecs}
                onRunRestoreRollback={submitRestoreRollback}
                onTargetClientIdChange={setRollbackTargetId}
                pending={pending}
                privilegeReady={Boolean(privilegeMaterial)}
                restoreJobId={rollbackRestoreJobId}
                restoreRollbackTimeoutSecs={rollbackTimeoutSecs}
                targetAgent={rollbackTarget}
                targetClientId={rollbackTargetId}
              />
              <PrivilegeVaultBox
                lastPayloadHash={lastPayloadHash}
                onOpenUnlock={onOpenPrivilegeUnlock}
                onPrivilegeMaterialChange={setPrivilegeMaterial}
                privilegeMaterial={privilegeMaterial}
              />
            </>
          )}
          {backupSubpage === "migration" && (
            <>
              <MigrationLinkForm
                archivePath={restoreArchivePath}
                clientLabel={clientLabel}
                forceUnprivileged={restoreForceUnprivileged}
                lastMigrationLink={lastMigrationLink}
                linkConfirmationOpen={pendingConfirmation === "migration-link"}
                migrationNote={migrationNote}
                migrationRestorePlanId={migrationRestorePlanId}
                onMigrationNoteChange={setMigrationNote}
                onMigrationRestorePlanIdChange={setMigrationRestorePlanId}
                onRunMigrationRestore={submitMigrationRun}
                onSubmit={submitMigrationLink}
                pending={pending}
                postRestoreArgv={restorePostRestoreArgv}
                privateKeyReady={Boolean(restorePrivateKeyHex.trim())}
                privilegeReady={Boolean(privilegeMaterial)}
                restoreDryRun={restoreDryRun}
                restorePlans={restorePlans}
                runConfirmationOpen={pendingConfirmation === "migration-run"}
                selectedPlan={selectedMigrationRestorePlan}
                sourceBackup={selectedMigrationSourceBackup}
              />
              <PrivilegeVaultBox
                lastPayloadHash={lastPayloadHash}
                onOpenUnlock={onOpenPrivilegeUnlock}
                onPrivilegeMaterialChange={setPrivilegeMaterial}
                privilegeMaterial={privilegeMaterial}
              />
            </>
          )}
        </div>
      </ConsoleActionDrawer>
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
