import { useEffect, useMemo, useState, type FormEvent, type ReactNode } from "react";
import { RefreshCw } from "lucide-react";
import { buildRestoreRollbackOperation } from "../backups/restoreRollback";
import { clampJobMaxTimeoutSecs, DEFAULT_MAX_JOB_TIMEOUT_SECS } from "../jobMaxTimeout";
import { ConfirmationPrompt } from "../components/ConfirmationPrompt";
import { ConsoleActionDrawer } from "../components/ConsoleLayout";
import { PrivilegeVaultBox } from "../components/PrivilegeVaultBox";
import { bytesToBase64 } from "../fileTransfer";
import { useReviewGenerationGuard, waitForReviewRender } from "../hooks/useReviewGenerationGuard";
import { usePanelDisplaySettings } from "../panelDisplay";
import {
  buildPrivilegeAssertion,
  buildPrivilegeForJobOperation,
  canonicalDbPrivilegeIntent,
  canonicalSchedulePrivilegeIntent,
  operationPayloadHashHex,
  parseCommandArgv,
  textPayloadHashHex,
  type PrivilegeMaterial,
} from "../privilege";
import {
  agentsMatchingExpression,
  parseSearchExpression,
  selectorExpressionForClientIds,
} from "../searchExpression";
import {
  DEFAULT_BACKUP_SELECTED_PATHS,
} from "../presets/backupPathPresets";
import { ArtifactUploadForm } from "./backups/ArtifactUploadForm";
import { BackupHistoryTables } from "./backups/BackupHistoryTables";
import { BackupPolicyForm } from "./backups/BackupPolicyForm";
import { BackupPolicyPruneForm } from "./backups/BackupPolicyPruneForm";
import { BackupRequestForm } from "./backups/BackupRequestForm";
import { MigrationLinkForm } from "./backups/MigrationLinkForm";
import type { RestoreArchiveTransferOption } from "./backups/RestoreArchiveTransferSelect";
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
  BulkResolveResponse,
  CreateBackupPolicyRequest,
  CreateBackupRequest,
  CreateJobRequest,
  CreateJobResponse,
  CreateMigrationLinkRequest,
  CreateMigrationRunRequest,
  CreateMigrationRunResponse,
  CreateRestorePlanRequest,
  JobOperation,
  JobOutputRecord,
  JobTargetSelection,
  MigrationLinkRecord,
  RestorePlanRecord,
  UploadBackupArtifactRequest,
} from "../types";
import type { FileTransferSessionRecord } from "../typesFileTransfer";
import {
  clientDisplayNameFromMap,
  clientDisplayNameMap,
  formatVpsName,
  runPanelAction,
  shortHash,
  shortId,
} from "../utils";

type BackupsPanelProps = {
  activeSubpage: string;
  agents: AgentView[];
  artifacts: BackupArtifactRecord[];
  backupPolicies: BackupPolicyRecord[];
  backups: BackupRequestRecord[];
  fileTransfers: FileTransferSessionRecord[];
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
  onCreateMigrationRun: (
    request: CreateMigrationRunRequest,
  ) => Promise<CreateMigrationRunResponse>;
  onCreateRestorePlan: (
    request: CreateRestorePlanRequest,
  ) => Promise<RestorePlanRecord>;
  onDownloadBackupArtifact: (backupRequestId: string) => Promise<Blob>;
  onHandoffBackupArtifact: (
    backupRequestId: string,
    request: BackupArtifactHandoffRequest,
  ) => Promise<BackupArtifactHandoffRecord>;
  onLoadJobOutputs: (jobId: string) => Promise<JobOutputRecord[]>;
  onPruneBackupPolicies: (
    request: BackupPolicyPruneRequest,
  ) => Promise<BackupPolicyPruneResponse>;
  onResolveTargets: (selection: JobTargetSelection) => Promise<BulkResolveResponse>;
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
  archiveTransfer: RestoreArchiveTransferOption | null;
  dryRun: boolean;
  postRestoreArgv: string;
  maxTimeoutSecs: number;
  forceUnprivileged: boolean;
};

type RestoreRunJobSnapshot = {
  payloadHashHex: string;
  request: CreateJobRequest;
  targetClientId: string;
};

type ConfirmationItem = { label: string; title?: string; value: ReactNode };

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

type BackupPolicySnapshot = {
  request: CreateBackupPolicyRequest;
  selectorExpression: string;
  targetClientIds: string[];
  targets: AgentView[];
};

type BackupActionSnapshot =
  | {
      action: "policy-prune";
      modeLabel: string;
      previewHash: string;
      request: BackupPolicyPruneRequest;
      reviewedRows: number;
      scopeLabel: string;
    }
  | {
      action: "backup-request";
      clientLabel: string;
      payloadHashHex: string;
      request: CreateBackupRequest;
      scopeLabel: string;
    }
  | {
      action: "artifact-upload";
      artifactBase64: string | null;
      backupRequestId: string;
      file: File;
      fileLabel: string;
      objectKey: string;
      requestLabel: string;
      uploadMode: "inline" | "chunked";
    }
  | {
      action: "artifact-handoff";
      backupRequestId: string;
      request: BackupArtifactHandoffRequest;
      requestLabel: string;
      sourceLabel: string;
    }
  | {
      action: "restore-plan";
      payloadHashHex: string;
      request: CreateRestorePlanRequest;
      scopeLabel: string;
      sourceLabel: string;
      targetLabel: string;
    }
  | {
      action: "restore-run";
      modeLabel: string;
      run: RestoreRunJobSnapshot;
      sourceLabel: string;
      targetLabel: string;
    }
  | {
      action: "restore-rollback";
      payloadHashHex: string;
      request: CreateJobRequest;
      restoreJobId: string;
      targetLabel: string;
    }
  | {
      action: "migration-link";
      noteLabel: string;
      payloadHashHex: string;
      planLabel: string;
      request: CreateMigrationLinkRequest;
    }
  | {
      action: "migration-run";
      linkRequest: CreateMigrationLinkRequest;
      linkPayloadHashHex: string;
      modeLabel: string;
      restorePlan: RestorePlanRecord;
      routeLabel: string;
      run: RestoreRunJobSnapshot;
    };

const INLINE_BACKUP_ARTIFACT_UPLOAD_LIMIT_BYTES = 16 * 1024 * 1024;
const NIL_UUID = "00000000-0000-0000-0000-000000000000";
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
  fileTransfers,
  migrationLinks,
  restorePlans,
  error,
  loading,
  onCreateBackupPolicy,
  onCreateBackupRequest,
  onCreateJob,
  onCreateMigrationLink,
  onCreateMigrationRun,
  onCreateRestorePlan,
  onDownloadBackupArtifact,
  onHandoffBackupArtifact,
  onLoadJobOutputs,
  onPruneBackupPolicies,
  onResolveTargets,
  onOpenPrivilegeUnlock,
  onUploadBackupArtifact,
  onUploadBackupArtifactChunked,
  onRefresh,
  privilegeMaterial,
  setPrivilegeMaterial,
}: BackupsPanelProps) {
  const { vpsNameDisplayMode } = usePanelDisplaySettings();
  const {
    captureReviewGeneration,
    invalidateReviewGeneration,
    isReviewGenerationCurrent,
  } = useReviewGenerationGuard();
  const [clientId, setClientId] = useState("");
  const [pathsText, setPathsText] = useState(DEFAULT_BACKUP_SELECTED_PATHS);
  const [includeConfig, setIncludeConfig] = useState(true);
  const [followSymlinks, setFollowSymlinks] = useState(false);
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
  const [policyFollowSymlinks, setPolicyFollowSymlinks] = useState(false);
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
  const [restoreNote, setRestoreNote] = useState("");
  const [restoreArchiveTransferKey, setRestoreArchiveTransferKey] = useState("");
  const [restoreDryRun, setRestoreDryRun] = useState(false);
  const [restorePostRestoreArgv, setRestorePostRestoreArgv] = useState("");
  const [restoreMaxTimeoutSecs, setRestoreMaxTimeoutSecs] = useState(60);
  const [restoreForceUnprivileged, setRestoreForceUnprivileged] =
    useState(false);
  const [rollbackRestoreJobId, setRollbackRestoreJobId] = useState("");
  const [rollbackTargetId, setRollbackTargetId] = useState("");
  const [rollbackMaxTimeoutSecs, setRollbackMaxTimeoutSecs] = useState(60);
  const [rollbackForceUnprivileged, setRollbackForceUnprivileged] =
    useState(false);
  const [lastRestorePlan, setLastRestorePlan] =
    useState<RestorePlanRecord | null>(null);
  const [lastRestoreJob, setLastRestoreJob] =
    useState<CreateJobResponse | null>(null);
  const [lastRollbackJob, setLastRollbackJob] =
    useState<CreateJobResponse | null>(null);
  const [migrationRestorePlanId, setMigrationRestorePlanId] = useState("");
  const [migrationArchiveTransferKey, setMigrationArchiveTransferKey] =
    useState("");
  const [migrationNote, setMigrationNote] = useState("");
  const [lastMigrationLink, setLastMigrationLink] =
    useState<MigrationLinkRecord | null>(null);
  const [pendingConfirmation, setPendingConfirmation] =
    useState<BackupConfirmationAction | null>(null);
  const [pendingPolicySnapshot, setPendingPolicySnapshot] =
    useState<BackupPolicySnapshot | null>(null);
  const [pendingActionSnapshot, setPendingActionSnapshot] =
    useState<BackupActionSnapshot | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [pending, setPending] = useState(false);
  const [reviewStatus, setReviewStatus] = useState<string | null>(null);
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
  const selectedRestoreSourceBackup =
    backups.find((backup) => backup.id === restoreSourceId) ?? null;
  const restorePaths = selectedRestoreSourceBackup?.paths ?? [];
  const restoreIncludeConfig =
    selectedRestoreSourceBackup?.include_config ?? false;
  const restoreDestinationRoot = generatedRestoreDestinationRoot(
    restoreSourceId,
    restoreTargetId,
  );
  const selectedRestoreSourceArtifact = backupArtifactForRequest(
    selectedRestoreSourceBackup,
    artifacts,
  );
  const restoreArchiveTransferOptions = useMemo(
    () =>
      buildRestoreArchiveTransferOptions(
        fileTransfers,
        selectedRestoreSourceBackup,
        selectedRestoreSourceArtifact,
        restoreTargetId,
      ),
    [
      artifacts,
      fileTransfers,
      restoreSourceId,
      restoreTargetId,
      selectedRestoreSourceArtifact,
      selectedRestoreSourceBackup,
    ],
  );
  const activeRestoreArchiveTransferKey = activeRestoreArchiveKey(
    restoreArchiveTransferKey,
    restoreArchiveTransferOptions,
  );
  const selectedRestoreArchiveTransfer =
    restoreArchiveTransferOptions.find(
      (option) => option.key === activeRestoreArchiveTransferKey,
    ) ?? null;
  const selectedMigrationSourceBackup =
    backups.find(
      (backup) =>
        backup.id === selectedMigrationRestorePlan?.source_backup_request_id,
    ) ?? null;
  const selectedMigrationSourceArtifact = backupArtifactForRequest(
    selectedMigrationSourceBackup,
    artifacts,
  );
  const migrationArchiveTransferOptions = useMemo(
    () =>
      buildRestoreArchiveTransferOptions(
        fileTransfers,
        selectedMigrationSourceBackup,
        selectedMigrationSourceArtifact,
        selectedMigrationRestorePlan?.target_client_id ?? "",
      ),
    [
      artifacts,
      fileTransfers,
      selectedMigrationRestorePlan,
      selectedMigrationSourceArtifact,
      selectedMigrationSourceBackup,
    ],
  );
  const activeMigrationArchiveTransferKey = activeRestoreArchiveKey(
    migrationArchiveTransferKey,
    migrationArchiveTransferOptions,
  );
  const selectedMigrationArchiveTransfer =
    migrationArchiveTransferOptions.find(
      (option) => option.key === activeMigrationArchiveTransferKey,
    ) ?? null;
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
  useEffect(() => {
    invalidateReviewGeneration();
    setActionError(null);
    setReviewStatus(null);
    setPendingConfirmation(null);
    setPendingPolicySnapshot(null);
    setPendingActionSnapshot(null);
  }, [backupSubpage, invalidateReviewGeneration]);
  const backupSubpageMeta = backupSubpageSummaries[backupSubpage];
  const status =
    actionError ??
    (reviewStatus ??
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
                      }`));

  async function runBackupReview(
    statusLabel: string,
    action: (reviewGeneration: number) => Promise<void>,
  ) {
    const reviewGeneration = captureReviewGeneration();
    setReviewStatus(statusLabel);
    try {
      await waitForReviewRender();
      await runPanelAction(setPending, setActionError, async () => {
        await action(reviewGeneration);
      });
    } finally {
      setReviewStatus(null);
    }
  }

  async function submitPolicy(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    await runBackupReview("Preparing backup policy review", async (reviewGeneration) => {
      if (!privilegeMaterial) {
        onOpenPrivilegeUnlock();
        throw new Error("Privilege unlock is locked");
      }
      if (!policyName.trim()) {
        throw new Error("Policy name is required");
      }
      if (!policyIncludeConfig && policyPaths.length === 0) {
        throw new Error("Select config or at least one absolute path");
      }
      if (policyTargetParse.error) {
        throw new Error(`Invalid target expression: ${policyTargetParse.error}`);
      }
      const selectorExpression = policyTargetsText.trim();
      if (!selectorExpression) {
        throw new Error("Add at least one target selector");
      }
      const resolved = await onResolveTargets({ selector_expression: selectorExpression });
      const targetClientIds = resolved.targets.map((target) => target.id);
      if (!targetClientIds.length) {
        throw new Error("Backup policy confirmation resolved no VPSs");
      }
      const operation: JobOperation = {
        type: "backup",
        paths: policyPaths,
        include_config: policyIncludeConfig,
        follow_symlinks: policyFollowSymlinks,
      };
      const operationPayloadHash = await operationPayloadHashHex(operation);
      const request: CreateBackupPolicyRequest = {
        name: policyName.trim(),
        selector_expression: selectorExpression,
        target_client_ids: targetClientIds,
        paths: policyPaths,
        include_config: policyIncludeConfig,
        follow_symlinks: policyFollowSymlinks,
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
        privilege_assertion: await buildPrivilegeAssertion({
          intent: canonicalSchedulePrivilegeIntent({
            action: "backup_policy.create",
            scheduleId: null,
            name: policyName.trim(),
            commandType: "backup",
            operationPayloadHash,
            selectorExpression,
            resolvedTargets: targetClientIds,
            cronExpr: policyCronExpr.trim(),
            timezone: "UTC",
            enabled: policyEnabled,
            catchUpPolicy: "skip_missed",
            catchUpLimit: 1,
            retryDelaySecs: 300,
        maxFailures: 3,
        deferredUntil: null,
        deleted: false,
      }),
      privilegeMaterial,
    }),
      };
      if (!isReviewGenerationCurrent(reviewGeneration)) {
        return;
      }
      setPendingPolicySnapshot({
        request,
        selectorExpression,
        targetClientIds,
        targets: resolved.targets,
      });
      setPendingConfirmation("policy");
    });
  }

  async function executePolicy() {
    await runPanelAction(setPending, setActionError, async () => {
      const snapshot = pendingPolicySnapshot;
      if (!snapshot) {
        throw new Error("Backup policy confirmation snapshot is missing; review the policy again");
      }
      const policy = await onCreateBackupPolicy(snapshot.request);
      setLastPolicy(policy);
      setPendingPolicySnapshot(null);
    });
  }

  function clearPolicyConfirmation() {
    invalidateReviewGeneration();
    setPendingPolicySnapshot(null);
    setPendingConfirmation((current) => (current === "policy" ? null : current));
  }

  function clearBackupConfirmations(actions: BackupConfirmationAction[]) {
    invalidateReviewGeneration();
    const actionSet = new Set(actions);
    if (actionSet.has("policy")) {
      setPendingPolicySnapshot(null);
    }
    setPendingActionSnapshot((current) =>
      current && actionSet.has(current.action) ? null : current,
    );
    setPendingConfirmation((current) =>
      current && actionSet.has(current) ? null : current,
    );
  }

  function policyPruneRequest(): BackupPolicyPruneRequest {
    return {
      schedule_id: policyPruneScheduleId || null,
      dry_run: policyPruneDryRun,
      metadata_only: policyPruneMetadataOnly,
      confirmed: !policyPruneDryRun,
    };
  }

  async function submitPolicyPrune(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const request = policyPruneRequest();
    if (policyPruneDryRun) {
      await executePolicyPrune(request);
    } else {
      await runPanelAction(setPending, setActionError, async () => {
        const previewRequest: BackupPolicyPruneRequest = {
          ...request,
          dry_run: true,
          confirmed: false,
          preview_hash: null,
        };
        const preview = await onPruneBackupPolicies(previewRequest);
        setLastPolicyPrune(preview);
        setPendingActionSnapshot({
          action: "policy-prune",
          modeLabel: policyPruneMetadataOnly
            ? "metadata only"
            : "metadata and objects",
          previewHash: preview.preview_hash,
          request: {
            ...request,
            dry_run: false,
            confirmed: true,
            preview_hash: preview.preview_hash,
          },
          reviewedRows: preview.policies.reduce(
            (sum, policy) => sum + policy.matched_rows,
            0,
          ),
          scopeLabel: policyPruneScheduleId
            ? shortId(policyPruneScheduleId)
            : "all policies",
        });
        setPendingConfirmation("policy-prune");
      });
    }
  }

  async function executePolicyPrune(request: BackupPolicyPruneRequest) {
    await runPanelAction(setPending, setActionError, async () => {
      const result = await onPruneBackupPolicies(request);
      setLastPolicyPrune(result);
      setPendingActionSnapshot((current) =>
        current?.action === "policy-prune" ? null : current,
      );
    });
  }

  async function submitRequest(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    await runBackupReview("Preparing backup request review", async (reviewGeneration) => {
      if (!privilegeMaterial) {
        onOpenPrivilegeUnlock();
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
        follow_symlinks: followSymlinks,
      };
      const selectorExpression = selectorExpressionForClientIds([clientId]);
      const built = await buildPrivilegeForJobOperation({
        clientIds: [clientId],
        commandType: "backup",
        operation,
        privilegeMaterial,
        selectorExpression,
        maxTimeoutSecs: DEFAULT_MAX_JOB_TIMEOUT_SECS,
      });
      if (!isReviewGenerationCurrent(reviewGeneration)) {
        return;
      }
      setPendingActionSnapshot({
        action: "backup-request",
        clientLabel: selectedAgent
          ? formatVpsName(selectedAgent, vpsNameDisplayMode)
          : clientId,
        payloadHashHex: built.payloadHashHex,
        request: {
          client_id: clientId,
          paths,
          include_config: includeConfig,
          follow_symlinks: followSymlinks,
          confirmed: true,
          note: note.trim() || null,
          privilege_assertion: built.privilegeAssertion,
        },
        scopeLabel: `${includeConfig ? "config, " : ""}${paths.length} paths, ${
          followSymlinks ? "follow symlinks" : "no symlink follow"
        }`,
      });
      setPendingConfirmation("backup-request");
    });
  }

  async function executeRequest(snapshot: Extract<BackupActionSnapshot, { action: "backup-request" }>) {
    await runPanelAction(setPending, setActionError, async () => {
      const request = await onCreateBackupRequest(snapshot.request);
      setLastPayloadHash(snapshot.payloadHashHex);
      setLastRequest(request);
      setArtifactBackupId(request.id);
      setArtifactObjectKey(`backups/${request.client_id}/${request.id}.tar`);
      setPendingActionSnapshot(null);
    });
  }

  async function submitArtifactUpload(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    await runBackupReview("Preparing artifact upload review", async (reviewGeneration) => {
      if (!artifactBackupId) {
        throw new Error("Select a backup request");
      }
      if (!artifactObjectKey.trim()) {
        throw new Error("Object key is required");
      }
      if (!artifactFile) {
        throw new Error("Select a backup artifact file");
      }
      const objectKey = artifactObjectKey.trim();
      const artifactBase64 =
        artifactUploadMode === "inline" ? await fileToBase64(artifactFile) : null;
      if (!isReviewGenerationCurrent(reviewGeneration)) {
        return;
      }
      setPendingActionSnapshot({
        action: "artifact-upload",
        artifactBase64,
        backupRequestId: artifactBackupId,
        file: artifactFile,
        fileLabel: `${artifactFile.name || "artifact"} (${artifactFile.size} bytes)`,
        objectKey,
        requestLabel: shortId(artifactBackupId),
        uploadMode: artifactUploadMode,
      });
      setPendingConfirmation("artifact-upload");
    });
  }

  async function executeArtifactUpload(snapshot: Extract<BackupActionSnapshot, { action: "artifact-upload" }>) {
    await runPanelAction(setPending, setActionError, async () => {
      const artifact =
        snapshot.uploadMode === "chunked"
          ? await onUploadBackupArtifactChunked(
              snapshot.backupRequestId,
              snapshot.objectKey,
              snapshot.file,
              true,
            )
          : await onUploadBackupArtifact(snapshot.backupRequestId, {
              object_key: snapshot.objectKey,
              artifact_base64: snapshot.artifactBase64 ?? "",
              confirmed: true,
            });
      setLastArtifact(artifact);
      setPendingActionSnapshot(null);
    });
  }

  function submitArtifactHandoff() {
    if (!artifactBackupId) {
      setActionError("Select a backup request");
      return;
    }
    setPendingActionSnapshot({
      action: "artifact-handoff",
      backupRequestId: artifactBackupId,
      request: {
        confirmed: true,
        job_id: handoffJobId.trim() || null,
      },
      requestLabel: shortId(artifactBackupId),
      sourceLabel: handoffJobId.trim()
        ? shortId(handoffJobId.trim())
        : "latest retained output",
    });
    setPendingConfirmation("artifact-handoff");
  }

  async function executeArtifactHandoff(snapshot: Extract<BackupActionSnapshot, { action: "artifact-handoff" }>) {
    await runPanelAction(setPending, setActionError, async () => {
      const handoff = await onHandoffBackupArtifact(
        snapshot.backupRequestId,
        snapshot.request,
      );
      setLastArtifact(handoff.artifact);
      setPendingActionSnapshot(null);
    });
  }

  function selectArtifactBackupId(backupId: string) {
    clearBackupConfirmations(["artifact-upload", "artifact-handoff"]);
    setArtifactBackupId(backupId);
    const backup = backups.find((item) => item.id === backupId);
    if (backup) {
      setArtifactObjectKey(`backups/${backup.client_id}/${backup.id}.tar`);
    }
  }

  function selectArtifactFile(file: File | null) {
    clearBackupConfirmations(["artifact-upload"]);
    setArtifactFile(file);
    if (file && file.size > INLINE_BACKUP_ARTIFACT_UPLOAD_LIMIT_BYTES) {
      setArtifactUploadMode("chunked");
    }
  }

  async function submitRestorePlan(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    await runBackupReview("Preparing restore plan review", async (reviewGeneration) => {
      if (!privilegeMaterial) {
        onOpenPrivilegeUnlock();
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
        archive_transfer_session_id: NIL_UUID,
        paths: restorePaths,
        include_config: restoreIncludeConfig,
        destination_root: restoreDestinationRoot.trim() || null,
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
        maxTimeoutSecs: DEFAULT_MAX_JOB_TIMEOUT_SECS,
      });
      if (!isReviewGenerationCurrent(reviewGeneration)) {
        return;
      }
      setPendingActionSnapshot({
        action: "restore-plan",
        payloadHashHex: built.payloadHashHex,
        request: {
          source_backup_request_id: restoreSourceId,
          target_client_id: restoreTargetId,
          paths: restorePaths,
          include_config: restoreIncludeConfig,
          destination_root: restoreDestinationRoot.trim() || null,
          confirmed: true,
          note: restoreNote.trim() || null,
          privilege_assertion: built.privilegeAssertion,
        },
        scopeLabel: `${restoreIncludeConfig ? "config, " : ""}${restorePaths.length} paths`,
        sourceLabel: shortId(restoreSourceId),
        targetLabel: restoreTarget
          ? formatVpsName(restoreTarget, vpsNameDisplayMode)
          : restoreTargetId,
      });
      setPendingConfirmation("restore-plan");
    });
  }

  async function executeRestorePlan(snapshot: Extract<BackupActionSnapshot, { action: "restore-plan" }>) {
    await runPanelAction(setPending, setActionError, async () => {
      const plan = await onCreateRestorePlan(snapshot.request);
      setLastPayloadHash(snapshot.payloadHashHex);
      setLastRestorePlan(plan);
      setPendingActionSnapshot(null);
    });
  }

  async function buildRestoreRunJobSnapshot(
    input: RestoreRunInput,
  ): Promise<RestoreRunJobSnapshot> {
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
    const archiveTransfer = input.archiveTransfer;
    if (!archiveTransfer) {
      throw new Error("Select a staged archive upload that matches the source backup artifact");
    }
    const archivePath = archiveTransfer.path.trim();
    const archiveSha256Hex = archiveTransfer.sha256Hex.trim().toLowerCase();
    if (!archivePath.startsWith("/")) {
      throw new Error("Selected staged archive path is not absolute");
    }
    const archiveSizeBytes = archiveTransfer.sizeBytes;
    if (!Number.isSafeInteger(archiveSizeBytes) || archiveSizeBytes <= 0) {
      throw new Error("Selected staged archive size is invalid");
    }
    if (!/^[0-9a-f]{64}$/.test(archiveSha256Hex)) {
      throw new Error("Selected staged archive SHA-256 is invalid");
    }
    const postRestoreArgv = input.postRestoreArgv.trim()
      ? parseCommandArgv(input.postRestoreArgv)
      : [];
    const operation: JobOperation = {
      type: "restore",
      source_backup_request_id: input.sourceBackupRequestId,
      archive_transfer_session_id: archiveTransfer.sessionId,
      paths: input.paths,
      include_config: input.includeConfig,
      destination_root: input.destinationRoot.trim() || null,
      archive_path: archivePath,
      archive_size_bytes: archiveSizeBytes,
      archive_sha256_hex: archiveSha256Hex,
      dry_run: input.dryRun,
      post_restore_argv: postRestoreArgv,
    };
    const selectorExpression = selectorExpressionForClientIds([
      input.targetClientId,
    ]);
    const boundedMaxTimeoutSecs = clampJobMaxTimeoutSecs(input.maxTimeoutSecs);
    const built = await buildPrivilegeForJobOperation({
      clientIds: [input.targetClientId],
      commandType: "restore",
      forceUnprivileged: input.forceUnprivileged,
      operation,
      privilegeMaterial,
      selectorExpression,
      maxTimeoutSecs: boundedMaxTimeoutSecs,
    });
    return {
      payloadHashHex: built.payloadHashHex,
      request: {
        selector_expression: selectorExpression,
        target_client_ids: [input.targetClientId],
        destructive: !input.dryRun,
        confirmed: true,
        command: "restore",
        argv: [],
        job_id: crypto.randomUUID(),
        operation,
        max_timeout_secs: boundedMaxTimeoutSecs,
        force_unprivileged: input.forceUnprivileged,
        privileged: true,
        privilege_assertion: built.privilegeAssertion,
      },
      targetClientId: input.targetClientId,
    };
  }

  async function executeRestoreRunSnapshot(snapshot: Extract<BackupActionSnapshot, { action: "restore-run" }>) {
    await runPanelAction(setPending, setActionError, async () => {
      const nextJob = await onCreateJob(snapshot.run.request);
      setLastPayloadHash(snapshot.run.payloadHashHex);
      setLastRestoreJob(nextJob);
      setRollbackRestoreJobId(nextJob.job_id);
      setRollbackTargetId(snapshot.run.targetClientId);
      setPendingActionSnapshot(null);
    });
  }

  async function executeRestoreRollbackSnapshot(snapshot: Extract<BackupActionSnapshot, { action: "restore-rollback" }>) {
    await runPanelAction(setPending, setActionError, async () => {
      const nextJob = await onCreateJob(snapshot.request);
      setLastPayloadHash(snapshot.payloadHashHex);
      setLastRollbackJob(nextJob);
      setPendingActionSnapshot(null);
    });
  }

  function buildRestoreRunInput(): RestoreRunInput {
    return {
      sourceBackupRequestId: restoreSourceId,
      targetClientId: restoreTargetId,
      paths: restorePaths,
      includeConfig: restoreIncludeConfig,
      destinationRoot: restoreDestinationRoot,
      archiveTransfer: selectedRestoreArchiveTransfer,
      dryRun: restoreDryRun,
      postRestoreArgv: restorePostRestoreArgv,
      maxTimeoutSecs: restoreMaxTimeoutSecs,
      forceUnprivileged: restoreForceUnprivileged,
    };
  }

  function restoreRunLabels(input: RestoreRunInput) {
    const target =
      agents.find((agent) => agent.id === input.targetClientId) ?? null;
    return {
      modeLabel: input.dryRun ? "dry run" : "live restore",
      sourceLabel: input.sourceBackupRequestId
        ? shortId(input.sourceBackupRequestId)
        : "none",
      targetLabel: target
        ? formatVpsName(target, vpsNameDisplayMode)
        : input.targetClientId || "none",
    };
  }

  async function submitRestoreRun() {
    await runBackupReview("Preparing restore run review", async (reviewGeneration) => {
      if (!privilegeMaterial) {
        onOpenPrivilegeUnlock();
        throw new Error("Privilege unlock is locked");
      }
      const input = buildRestoreRunInput();
      const run = await buildRestoreRunJobSnapshot(input);
      if (!isReviewGenerationCurrent(reviewGeneration)) {
        return;
      }
      setPendingActionSnapshot({
        action: "restore-run",
        run,
        ...restoreRunLabels(input),
      });
      setPendingConfirmation("restore-run");
    });
  }

  async function submitRestoreRollback() {
    await runBackupReview("Preparing restore rollback review", async (reviewGeneration) => {
      if (!privilegeMaterial) {
        onOpenPrivilegeUnlock();
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
      const boundedMaxTimeoutSecs = clampJobMaxTimeoutSecs(rollbackMaxTimeoutSecs);
      const built = await buildPrivilegeForJobOperation({
        clientIds: [targetClientId],
        commandType: "restore_rollback",
        forceUnprivileged: rollbackForceUnprivileged,
        operation,
        privilegeMaterial,
        selectorExpression,
        maxTimeoutSecs: boundedMaxTimeoutSecs,
      });
      if (!isReviewGenerationCurrent(reviewGeneration)) {
        return;
      }
      setPendingActionSnapshot({
        action: "restore-rollback",
        payloadHashHex: built.payloadHashHex,
        request: {
          selector_expression: selectorExpression,
          target_client_ids: [targetClientId],
          destructive: true,
          confirmed: true,
          command: "restore_rollback",
          argv: [],
          job_id: crypto.randomUUID(),
          operation,
          max_timeout_secs: boundedMaxTimeoutSecs,
          force_unprivileged: rollbackForceUnprivileged,
          privileged: true,
          privilege_assertion: built.privilegeAssertion,
        },
        restoreJobId,
        targetLabel: rollbackTarget
          ? formatVpsName(rollbackTarget, vpsNameDisplayMode)
          : targetClientId,
      });
      setPendingConfirmation("restore-rollback");
    });
  }

  async function buildMigrationLinkReview(
    restorePlan: RestorePlanRecord,
  ): Promise<{
    payloadHashHex: string;
    request: CreateMigrationLinkRequest;
  }> {
    if (!privilegeMaterial) {
      onOpenPrivilegeUnlock();
      throw new Error("Privilege unlock is locked");
    }
    const note = migrationNote.trim() || null;
    const payloadHashHex = await migrationLinkPayloadHashHex(restorePlan, note);
    return {
      payloadHashHex,
      request: {
        restore_plan_id: restorePlan.id,
        confirmed: true,
        note,
        privilege_assertion: await buildPrivilegeAssertion({
          intent: canonicalDbPrivilegeIntent({
            action: "migration.link",
            target: restorePlan.id,
            selectorExpression: null,
            resolvedTargets: [
              restorePlan.source_client_id,
              restorePlan.target_client_id,
            ],
            confirmed: true,
            payloadHash: payloadHashHex,
          }),
          privilegeMaterial,
        }),
      },
    };
  }

  async function submitMigrationLink() {
    await runBackupReview("Preparing migration link review", async (reviewGeneration) => {
      const restorePlan = selectedMigrationRestorePlan;
      if (!restorePlan) {
        throw new Error("Select a restore plan");
      }
      const review = await buildMigrationLinkReview(restorePlan);
      if (!isReviewGenerationCurrent(reviewGeneration)) {
        return;
      }
      setPendingActionSnapshot({
        action: "migration-link",
        noteLabel: migrationNote.trim() || "none",
        payloadHashHex: review.payloadHashHex,
        planLabel: shortId(restorePlan.id),
        request: review.request,
      });
      setPendingConfirmation("migration-link");
    });
  }

  async function executeMigrationLink(snapshot: Extract<BackupActionSnapshot, { action: "migration-link" }>) {
    await runPanelAction(setPending, setActionError, async () => {
      const link = await onCreateMigrationLink(snapshot.request);
      setLastMigrationLink(link);
      setPendingActionSnapshot(null);
    });
  }

  async function submitMigrationRun() {
    await runBackupReview("Preparing migration restore review", async (reviewGeneration) => {
      if (!privilegeMaterial) {
        onOpenPrivilegeUnlock();
        throw new Error("Privilege unlock is locked");
      }
      const restorePlan = selectedMigrationRestorePlan;
      if (!restorePlan) {
        throw new Error("Select a restore plan");
      }
      const input: RestoreRunInput = {
        sourceBackupRequestId: restorePlan.source_backup_request_id,
        targetClientId: restorePlan.target_client_id,
        paths: restorePlan.paths,
        includeConfig: restorePlan.include_config,
        destinationRoot: restorePlan.destination_root ?? "",
        archiveTransfer: selectedMigrationArchiveTransfer,
        dryRun: restoreDryRun,
        postRestoreArgv: restorePostRestoreArgv,
        maxTimeoutSecs: restoreMaxTimeoutSecs,
        forceUnprivileged: restoreForceUnprivileged,
      };
      const run = await buildRestoreRunJobSnapshot(input);
      const linkReview = await buildMigrationLinkReview(restorePlan);
      if (!isReviewGenerationCurrent(reviewGeneration)) {
        return;
      }
      setPendingActionSnapshot({
        action: "migration-run",
        linkRequest: linkReview.request,
        linkPayloadHashHex: linkReview.payloadHashHex,
        modeLabel: input.dryRun ? "dry run" : "live restore",
        restorePlan,
        routeLabel: `${clientLabel(restorePlan.source_client_id)} to ${clientLabel(restorePlan.target_client_id)}`,
        run,
      });
      setPendingConfirmation("migration-run");
    });
  }

  async function executeMigrationRun(snapshot: Extract<BackupActionSnapshot, { action: "migration-run" }>) {
    await runPanelAction(setPending, setActionError, async () => {
      const response = await onCreateMigrationRun({
        link: snapshot.linkRequest,
        job: snapshot.run.request,
      });
      const link = response.migration_link;
      const nextJob = response.restore_job;
      setRestoreSourceId(snapshot.restorePlan.source_backup_request_id);
      setRestoreTargetId(snapshot.restorePlan.target_client_id);
      setRestoreArchiveTransferKey("");
      setLastMigrationLink(link);
      setLastPayloadHash(snapshot.run.payloadHashHex);
      setLastRestoreJob(nextJob);
      setRollbackRestoreJobId(nextJob.job_id);
      setRollbackTargetId(snapshot.restorePlan.target_client_id);
      setPendingActionSnapshot(null);
    });
  }

  function restoreArchiveConfirmationItems(
    run: RestoreRunJobSnapshot | null,
  ): ConfirmationItem[] {
    const operation = run?.request.operation;
    const restoreOperation = operation?.type === "restore" ? operation : null;
    const fallbackArchive =
      backupSubpage === "migration"
        ? selectedMigrationArchiveTransfer
        : selectedRestoreArchiveTransfer;
    const archivePath =
      restoreOperation?.archive_path ?? fallbackArchive?.path;
    const archiveSizeBytes =
      restoreOperation?.archive_size_bytes ?? fallbackArchive?.sizeBytes;
    const archiveSha256Hex =
      restoreOperation?.archive_sha256_hex ?? fallbackArchive?.sha256Hex;
    const archiveTransferSessionId =
      restoreOperation?.archive_transfer_session_id ?? fallbackArchive?.sessionId;
    return [
      {
        label: "Archive transfer",
        value: archiveTransferSessionId ? shortId(archiveTransferSessionId) : "missing",
        title: archiveTransferSessionId ?? "missing",
      },
      { label: "Archive path", value: archivePath || "missing" },
      { label: "Archive size", value: archiveSizeBytes || "missing" },
      {
        label: "Archive SHA-256",
        value: archiveSha256Hex ? shortHash(archiveSha256Hex) : "missing",
        title: archiveSha256Hex ?? "missing",
      },
    ];
  }

  function buildBackupConfirmationItems(
    action: BackupConfirmationAction,
  ): ConfirmationItem[] {
    switch (action) {
      case "policy": {
        const policySnapshot = pendingPolicySnapshot;
        return [
          {
            label: "Policy",
            value: policySnapshot?.request.name ?? policyName.trim() ?? "unnamed",
          },
          {
            label: "Fixed targets",
            value: policySnapshot
              ? `${policySnapshot.targetClientIds.length} VPSs resolved and saved`
              : `${policyTargetCount} VPSs resolved and saved`,
          },
          {
            label: "Scope",
            value: policySnapshot
              ? `${policySnapshot.request.include_config ? "config, " : ""}${policySnapshot.request.paths.length} paths, ${
                  policySnapshot.request.follow_symlinks ? "follow symlinks" : "no symlink follow"
                }`
              : `${policyIncludeConfig ? "config, " : ""}${policyPaths.length} paths, ${
                  policyFollowSymlinks ? "follow symlinks" : "no symlink follow"
                }`,
          },
          {
            label: "Schedule",
            value:
              policySnapshot?.request.cron_expr ??
              policyCronExpr.trim() ??
              "cron required",
          },
          {
            label: "Preview",
            value: policySnapshot
              ? policySnapshot.targets
                  .slice(0, 4)
                  .map((target) => formatVpsName(target, vpsNameDisplayMode))
                  .join(", ") +
                (policySnapshot.targets.length > 4
                  ? `, +${policySnapshot.targets.length - 4} more`
                  : "")
              : "Review policy to freeze targets",
          },
        ];
      }
      case "policy-prune": {
        const snapshot =
          pendingActionSnapshot?.action === "policy-prune"
            ? pendingActionSnapshot
            : null;
        return [
          {
            label: "Scope",
            value:
              snapshot?.scopeLabel ??
              (policyPruneScheduleId
                ? shortId(policyPruneScheduleId)
                : "all policies"),
          },
          {
            label: "Mode",
            value:
              snapshot?.modeLabel ??
              (policyPruneMetadataOnly
                ? "metadata only"
                : "metadata and objects"),
          },
          {
            label: "Reviewed rows",
            value: snapshot?.reviewedRows ?? 0,
          },
          {
            label: "Review hash",
            value: snapshot ? `${snapshot.previewHash.slice(0, 12)}...` : "review required",
          },
        ];
      }
      case "backup-request": {
        const snapshot =
          pendingActionSnapshot?.action === "backup-request"
            ? pendingActionSnapshot
            : null;
        return [
          {
            label: "VPS",
            value:
              snapshot?.clientLabel ??
              (selectedAgent
                ? formatVpsName(selectedAgent, vpsNameDisplayMode)
                : clientId || "none"),
          },
          {
            label: "Scope",
            value:
              snapshot?.scopeLabel ??
              `${includeConfig ? "config, " : ""}${paths.length} paths`,
          },
          {
            label: "Privilege",
            value: snapshot ? "Frozen assertion" : "Review backup to freeze",
          },
        ];
      }
      case "artifact-upload": {
        const snapshot =
          pendingActionSnapshot?.action === "artifact-upload"
            ? pendingActionSnapshot
            : null;
        return [
          {
            label: "Request",
            value:
              snapshot?.requestLabel ??
              (artifactBackupId ? shortId(artifactBackupId) : "none"),
          },
          {
            label: "Object",
            value: snapshot?.objectKey ?? (artifactObjectKey.trim() || "missing"),
          },
          { label: "Mode", value: snapshot?.uploadMode ?? artifactUploadMode },
          { label: "File", value: snapshot?.fileLabel ?? "Review upload to freeze" },
        ];
      }
      case "artifact-handoff": {
        const snapshot =
          pendingActionSnapshot?.action === "artifact-handoff"
            ? pendingActionSnapshot
            : null;
        return [
          {
            label: "Request",
            value:
              snapshot?.requestLabel ??
              (artifactBackupId ? shortId(artifactBackupId) : "none"),
          },
          {
            label: "Source job",
            value:
              snapshot?.sourceLabel ??
              (handoffJobId.trim() || "latest retained output"),
          },
        ];
      }
      case "restore-plan": {
        const snapshot =
          pendingActionSnapshot?.action === "restore-plan"
            ? pendingActionSnapshot
            : null;
        return [
          {
            label: "Source",
            value:
              snapshot?.sourceLabel ??
              (restoreSourceId ? shortId(restoreSourceId) : "none"),
          },
          {
            label: "Target",
            value:
              snapshot?.targetLabel ??
              (restoreTarget
                ? formatVpsName(restoreTarget, vpsNameDisplayMode)
                : restoreTargetId || "none"),
          },
          {
            label: "Scope",
            value:
              snapshot?.scopeLabel ??
              `${restoreIncludeConfig ? "config, " : ""}${restorePaths.length} paths`,
          },
          {
            label: "Privilege",
            value: snapshot ? "Frozen assertion" : "Review plan to freeze",
          },
        ];
      }
      case "restore-run": {
        const snapshot =
          pendingActionSnapshot?.action === "restore-run"
            ? pendingActionSnapshot
            : null;
        return [
          {
            label: "Source",
            value:
              snapshot?.sourceLabel ??
              (restoreSourceId ? shortId(restoreSourceId) : "none"),
          },
          {
            label: "Target",
            value:
              snapshot?.targetLabel ??
              (restoreTarget
                ? formatVpsName(restoreTarget, vpsNameDisplayMode)
                : restoreTargetId || "none"),
          },
          {
            label: "Mode",
            value: snapshot?.modeLabel ?? (restoreDryRun ? "dry run" : "live restore"),
          },
          ...restoreArchiveConfirmationItems(snapshot?.run ?? null),
          {
            label: "Privilege",
            value: snapshot ? "Frozen assertion" : "Review restore to freeze",
          },
        ];
      }
      case "restore-rollback": {
        const snapshot =
          pendingActionSnapshot?.action === "restore-rollback"
            ? pendingActionSnapshot
            : null;
        return [
          {
            label: "Restore job",
            value:
              snapshot?.restoreJobId ??
              (rollbackRestoreJobId.trim()
                ? shortId(rollbackRestoreJobId.trim())
                : "none"),
          },
          {
            label: "Target",
            value:
              snapshot?.targetLabel ??
              (rollbackTarget
                ? formatVpsName(rollbackTarget, vpsNameDisplayMode)
                : rollbackTargetId || "none"),
          },
          {
            label: "Privilege",
            value: snapshot ? "Frozen assertion" : "Review rollback to freeze",
          },
        ];
      }
      case "migration-link": {
        const snapshot =
          pendingActionSnapshot?.action === "migration-link"
            ? pendingActionSnapshot
            : null;
        return [
          {
            label: "Restore plan",
            value:
              snapshot?.planLabel ??
              (migrationRestorePlanId ? shortId(migrationRestorePlanId) : "none"),
          },
          { label: "Note", value: snapshot?.noteLabel ?? (migrationNote.trim() || "none") },
          {
            label: "Link hash",
            value: snapshot ? `${snapshot.payloadHashHex.slice(0, 12)}...` : "review required",
          },
        ];
      }
      case "migration-run": {
        const snapshot =
          pendingActionSnapshot?.action === "migration-run"
            ? pendingActionSnapshot
            : null;
        return [
          {
            label: "Restore plan",
            value:
              (snapshot ? shortId(snapshot.restorePlan.id) : null) ??
              (selectedMigrationRestorePlan
                ? shortId(selectedMigrationRestorePlan.id)
                : "none"),
          },
          {
            label: "Route",
            value:
              snapshot?.routeLabel ??
              (selectedMigrationRestorePlan
                ? `${clientLabel(selectedMigrationRestorePlan.source_client_id)} to ${clientLabel(selectedMigrationRestorePlan.target_client_id)}`
                : "none"),
          },
          {
            label: "Mode",
            value: snapshot?.modeLabel ?? (restoreDryRun ? "dry run" : "live restore"),
          },
          ...restoreArchiveConfirmationItems(snapshot?.run ?? null),
          {
            label: "Privilege",
            value: snapshot ? "Frozen assertion" : "Review migration to freeze",
          },
          {
            label: "Link hash",
            value: snapshot ? `${snapshot.linkPayloadHashHex.slice(0, 12)}...` : "review required",
          },
        ];
      }
    }
  }

  function backupConfirmationDetail(
    action: BackupConfirmationAction | null,
  ): string {
    switch (action) {
      case "policy":
        return "Confirm the saved schedule, target snapshot, and backup scope.";
      case "policy-prune":
        return (pendingActionSnapshot?.action === "policy-prune"
          ? pendingActionSnapshot.request.metadata_only
          : policyPruneMetadataOnly)
          ? "Confirm pruning retained backup metadata for the selected policy scope."
          : "Confirm pruning retained backup metadata and deleting retained object files for the selected policy scope.";
      case "backup-request":
        return "Confirm this browser-unlocked backup request before it is written.";
      case "artifact-upload":
        return "Confirm the backup artifact upload for the selected backup request.";
      case "artifact-handoff":
        return "Confirm promoting retained job output into a backup artifact record.";
      case "restore-plan":
        return "Confirm the restore intent and target before saving the plan.";
      case "restore-run":
        return (pendingActionSnapshot?.action === "restore-run"
          ? !pendingActionSnapshot.run.request.destructive
          : restoreDryRun)
          ? "Confirm the restore rehearsal dispatch."
          : "Confirm the live restore dispatch.";
      case "restore-rollback":
        return "Confirm restore rollback dispatch for the selected target.";
      case "migration-link":
        return "Confirm writing the migration link for the selected restore plan.";
      case "migration-run":
        return (pendingActionSnapshot?.action === "migration-run"
          ? !pendingActionSnapshot.run.request.destructive
          : restoreDryRun)
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
      case "policy-prune": {
        if (pendingActionSnapshot?.action !== "policy-prune") {
          setActionError("Backup prune confirmation snapshot is missing; review prune again");
          return;
        }
        await executePolicyPrune(pendingActionSnapshot.request);
        break;
      }
      case "backup-request": {
        if (pendingActionSnapshot?.action !== "backup-request") {
          setActionError("Backup request confirmation snapshot is missing; review backup again");
          return;
        }
        await executeRequest(pendingActionSnapshot);
        break;
      }
      case "artifact-upload": {
        if (pendingActionSnapshot?.action !== "artifact-upload") {
          setActionError("Artifact upload confirmation snapshot is missing; review upload again");
          return;
        }
        await executeArtifactUpload(pendingActionSnapshot);
        break;
      }
      case "artifact-handoff": {
        if (pendingActionSnapshot?.action !== "artifact-handoff") {
          setActionError("Artifact handoff confirmation snapshot is missing; review promotion again");
          return;
        }
        await executeArtifactHandoff(pendingActionSnapshot);
        break;
      }
      case "restore-plan": {
        if (pendingActionSnapshot?.action !== "restore-plan") {
          setActionError("Restore plan confirmation snapshot is missing; review plan again");
          return;
        }
        await executeRestorePlan(pendingActionSnapshot);
        break;
      }
      case "restore-run": {
        if (pendingActionSnapshot?.action !== "restore-run") {
          setActionError("Restore run confirmation snapshot is missing; review restore again");
          return;
        }
        await executeRestoreRunSnapshot(pendingActionSnapshot);
        break;
      }
      case "restore-rollback": {
        if (pendingActionSnapshot?.action !== "restore-rollback") {
          setActionError("Restore rollback confirmation snapshot is missing; review rollback again");
          return;
        }
        await executeRestoreRollbackSnapshot(pendingActionSnapshot);
        break;
      }
      case "migration-link": {
        if (pendingActionSnapshot?.action !== "migration-link") {
          setActionError("Migration link confirmation snapshot is missing; review link again");
          return;
        }
        await executeMigrationLink(pendingActionSnapshot);
        break;
      }
      case "migration-run": {
        if (pendingActionSnapshot?.action !== "migration-run") {
          setActionError("Migration run confirmation snapshot is missing; review migration again");
          return;
        }
        await executeMigrationRun(pendingActionSnapshot);
        break;
      }
    }
  }

  const backupConfirmationTitle =
    pendingConfirmation === "policy"
      ? "Confirm backup policy"
      : pendingConfirmation === "policy-prune"
        ? "Confirm backup artifact prune"
        : pendingConfirmation === "backup-request"
          ? "Confirm backup request"
          : pendingConfirmation === "artifact-upload"
            ? "Confirm backup artifact upload"
            : pendingConfirmation === "artifact-handoff"
              ? "Confirm retained output promotion"
              : pendingConfirmation === "restore-plan"
                ? "Confirm restore plan"
                : pendingConfirmation === "restore-run"
                  ? "Confirm restore run"
                  : pendingConfirmation === "restore-rollback"
                    ? "Confirm restore rollback"
                    : pendingConfirmation === "migration-link"
                      ? "Confirm migration link"
                      : "Confirm migration restore";
  const backupConfirmationConfirmLabel =
    pendingConfirmation === "policy"
      ? "Save policy"
      : pendingConfirmation === "policy-prune"
        ? "Prune artifacts"
        : pendingConfirmation === "backup-request"
          ? "Request backup"
          : pendingConfirmation === "artifact-upload"
            ? "Upload artifact"
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
            confirmLabel={backupConfirmationConfirmLabel}
            detail={backupConfirmationDetail(pendingConfirmation)}
            items={backupConfirmationItems}
            onCancel={() => {
              if (pendingConfirmation === "policy") {
                setPendingPolicySnapshot(null);
              } else {
                setPendingActionSnapshot(null);
              }
              setPendingConfirmation(null);
            }}
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
                followSymlinks={policyFollowSymlinks}
                includeConfig={policyIncludeConfig}
                keepLast={policyKeepLast}
                name={policyName}
                onCronExprChange={(value) => {
                  setPolicyCronExpr(value);
                  clearPolicyConfirmation();
                }}
                onEnabledChange={(value) => {
                  setPolicyEnabled(value);
                  clearPolicyConfirmation();
                }}
                onFollowSymlinksChange={(value) => {
                  setPolicyFollowSymlinks(value);
                  clearPolicyConfirmation();
                }}
                onIncludeConfigChange={(value) => {
                  setPolicyIncludeConfig(value);
                  clearPolicyConfirmation();
                }}
                onKeepLastChange={(value) => {
                  setPolicyKeepLast(value);
                  clearPolicyConfirmation();
                }}
                onNameChange={(value) => {
                  setPolicyName(value);
                  clearPolicyConfirmation();
                }}
                onPathsTextChange={(value) => {
                  setPolicyPathsText(value);
                  clearPolicyConfirmation();
                }}
                onRetentionDaysChange={(value) => {
                  setPolicyRetentionDays(value);
                  clearPolicyConfirmation();
                }}
                onRotationGenerationChange={(value) => {
                  setPolicyRotationGeneration(value);
                  clearPolicyConfirmation();
                }}
                onSubmit={submitPolicy}
                onTargetsTextChange={(value) => {
                  setPolicyTargetsText(value);
                  clearPolicyConfirmation();
                }}
                pathsCount={policyPaths.length}
                pathsText={policyPathsText}
                pending={pending}
                policyEnabled={policyEnabled}
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
                onDryRunChange={(value) => {
                  setPolicyPruneDryRun(value);
                  clearBackupConfirmations(["policy-prune"]);
                }}
                onMetadataOnlyChange={(value) => {
                  setPolicyPruneMetadataOnly(value);
                  clearBackupConfirmations(["policy-prune"]);
                }}
                onScheduleIdChange={(value) => {
                  setPolicyPruneScheduleId(value);
                  clearBackupConfirmations(["policy-prune"]);
                }}
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
              followSymlinks={followSymlinks}
              includeConfig={includeConfig}
              note={note}
              onClientIdChange={(value) => {
                setClientId(value);
                clearBackupConfirmations(["backup-request"]);
              }}
              onFollowSymlinksChange={(value) => {
                setFollowSymlinks(value);
                clearBackupConfirmations(["backup-request"]);
              }}
              onIncludeConfigChange={(value) => {
                setIncludeConfig(value);
                clearBackupConfirmations(["backup-request"]);
              }}
              onNoteChange={(value) => {
                setNote(value);
                clearBackupConfirmations(["backup-request"]);
              }}
              onPathsTextChange={(value) => {
                setPathsText(value);
                clearBackupConfirmations(["backup-request"]);
              }}
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
              onArtifactObjectKeyChange={(value) => {
                setArtifactObjectKey(value);
                clearBackupConfirmations(["artifact-upload"]);
              }}
              onArtifactUploadModeChange={(value) => {
                setArtifactUploadMode(value);
                clearBackupConfirmations(["artifact-upload"]);
              }}
              onHandoffJobIdChange={(value) => {
                setHandoffJobId(value);
                clearBackupConfirmations(["artifact-handoff"]);
              }}
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
                onNoteChange={(value) => {
                  setRestoreNote(value);
                  clearBackupConfirmations(["restore-plan"]);
                }}
                onSourceIdChange={(value) => {
                  setRestoreSourceId(value);
                  setRestoreArchiveTransferKey("");
                  clearBackupConfirmations([
                    "restore-plan",
                    "restore-run",
                    "migration-run",
                  ]);
                }}
                onSubmit={submitRestorePlan}
                onTargetIdChange={(value) => {
                  setRestoreTargetId(value);
                  setRestoreArchiveTransferKey("");
                  clearBackupConfirmations([
                    "restore-plan",
                    "restore-run",
                    "migration-run",
                  ]);
                }}
                pending={pending}
                privilegeReady={Boolean(privilegeMaterial)}
                restoreDestinationRoot={restoreDestinationRoot}
                restoreIncludeConfig={restoreIncludeConfig}
                restoreNote={restoreNote}
                restorePaths={restorePaths}
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
                archiveEmptyMessage={restoreArchiveEmptyMessage(
                  restoreSourceId,
                  restoreTargetId,
                  selectedRestoreSourceBackup,
                  selectedRestoreSourceArtifact,
                )}
                archiveTransferKey={activeRestoreArchiveTransferKey}
                archiveTransferOptions={restoreArchiveTransferOptions}
                confirmationOpen={pendingConfirmation === "restore-run"}
                forceUnprivileged={restoreForceUnprivileged}
                onArchiveTransferChange={(value) => {
                  setRestoreArchiveTransferKey(value);
                  clearBackupConfirmations(["restore-run"]);
                }}
                onForceUnprivilegedChange={(value) => {
                  setRestoreForceUnprivileged(value);
                  clearBackupConfirmations(["restore-run", "migration-run"]);
                }}
                onDryRunChange={(value) => {
                  setRestoreDryRun(value);
                  clearBackupConfirmations(["restore-run", "migration-run"]);
                }}
                onPostRestoreArgvChange={(value) => {
                  setRestorePostRestoreArgv(value);
                  clearBackupConfirmations(["restore-run", "migration-run"]);
                }}
                onRestoreMaxTimeoutSecsChange={(value) => {
                  setRestoreMaxTimeoutSecs(value);
                  clearBackupConfirmations(["restore-run", "migration-run"]);
                }}
                onRunRestore={submitRestoreRun}
                pending={pending}
                privilegeReady={Boolean(privilegeMaterial)}
                restoreDryRun={restoreDryRun}
                restorePostRestoreArgv={restorePostRestoreArgv}
                restoreSourceId={restoreSourceId}
                restoreTarget={restoreTarget}
                restoreTargetId={restoreTargetId}
                restoreMaxTimeoutSecs={restoreMaxTimeoutSecs}
              />
              <RestoreRollbackForm
                agents={agents}
                confirmationOpen={pendingConfirmation === "restore-rollback"}
                forceUnprivileged={rollbackForceUnprivileged}
                onForceUnprivilegedChange={(value) => {
                  setRollbackForceUnprivileged(value);
                  clearBackupConfirmations(["restore-rollback"]);
                }}
                onRestoreJobIdChange={(value) => {
                  setRollbackRestoreJobId(value);
                  clearBackupConfirmations(["restore-rollback"]);
                }}
                onRestoreRollbackMaxTimeoutSecsChange={(value) => {
                  setRollbackMaxTimeoutSecs(value);
                  clearBackupConfirmations(["restore-rollback"]);
                }}
                onRunRestoreRollback={submitRestoreRollback}
                onTargetClientIdChange={(value) => {
                  setRollbackTargetId(value);
                  clearBackupConfirmations(["restore-rollback"]);
                }}
                pending={pending}
                privilegeReady={Boolean(privilegeMaterial)}
                restoreJobId={rollbackRestoreJobId}
                restoreRollbackMaxTimeoutSecs={rollbackMaxTimeoutSecs}
                targetAgent={rollbackTarget}
                targetClientId={rollbackTargetId}
              />
              <PrivilegeVaultBox
                lastPayloadHash={lastPayloadHash}
                onOpenUnlock={onOpenPrivilegeUnlock}
                onPrivilegeMaterialChange={(material) => {
                  setPrivilegeMaterial(material);
                  clearBackupConfirmations([
                    "restore-plan",
                    "restore-run",
                    "restore-rollback",
                  ]);
                }}
                privilegeMaterial={privilegeMaterial}
              />
            </>
          )}
          {backupSubpage === "migration" && (
            <>
              <MigrationLinkForm
                archiveEmptyMessage={restoreArchiveEmptyMessage(
                  selectedMigrationRestorePlan?.source_backup_request_id ?? "",
                  selectedMigrationRestorePlan?.target_client_id ?? "",
                  selectedMigrationSourceBackup,
                  selectedMigrationSourceArtifact,
                )}
                archiveTransferKey={activeMigrationArchiveTransferKey}
                archiveTransferOptions={migrationArchiveTransferOptions}
                clientLabel={clientLabel}
                forceUnprivileged={restoreForceUnprivileged}
                lastMigrationLink={lastMigrationLink}
                linkConfirmationOpen={pendingConfirmation === "migration-link"}
                migrationNote={migrationNote}
                migrationRestorePlanId={migrationRestorePlanId}
                onArchiveTransferChange={(value) => {
                  setMigrationArchiveTransferKey(value);
                  clearBackupConfirmations(["migration-run"]);
                }}
                onMigrationNoteChange={(value) => {
                  setMigrationNote(value);
                  clearBackupConfirmations(["migration-link", "migration-run"]);
                }}
                onMigrationRestorePlanIdChange={(value) => {
                  setMigrationRestorePlanId(value);
                  setMigrationArchiveTransferKey("");
                  clearBackupConfirmations(["migration-link", "migration-run"]);
                }}
                onRunMigrationRestore={submitMigrationRun}
                onSubmit={submitMigrationLink}
                pending={pending}
                postRestoreArgv={restorePostRestoreArgv}
                privilegeReady={Boolean(privilegeMaterial)}
                restoreDryRun={restoreDryRun}
                restorePlans={restorePlans}
                runConfirmationOpen={pendingConfirmation === "migration-run"}
                selectedPlan={selectedMigrationRestorePlan}
              />
              <PrivilegeVaultBox
                lastPayloadHash={lastPayloadHash}
                onOpenUnlock={onOpenPrivilegeUnlock}
                onPrivilegeMaterialChange={(material) => {
                  setPrivilegeMaterial(material);
                  clearBackupConfirmations(["migration-link", "migration-run"]);
                }}
                privilegeMaterial={privilegeMaterial}
              />
            </>
          )}
        </div>
      </ConsoleActionDrawer>
    </section>
  );
}

function backupArtifactForRequest(
  backup: BackupRequestRecord | null,
  artifacts: BackupArtifactRecord[],
): BackupArtifactRecord | null {
  if (!backup?.artifact_id) {
    return null;
  }
  return artifacts.find((artifact) => artifact.id === backup.artifact_id) ?? null;
}

function migrationLinkPayloadHashHex(
  restorePlan: RestorePlanRecord,
  note: string | null,
): Promise<string> {
  return textPayloadHashHex(
    JSON.stringify({
      destination_root: restorePlan.destination_root ?? null,
      include_config: restorePlan.include_config,
      note,
      paths: restorePlan.paths,
      restore_plan_id: restorePlan.id,
      source_backup_request_id: restorePlan.source_backup_request_id,
      source_client_id: restorePlan.source_client_id,
      target_client_id: restorePlan.target_client_id,
      version: 1,
    }),
  );
}

function buildRestoreArchiveTransferOptions(
  transfers: FileTransferSessionRecord[],
  sourceBackup: BackupRequestRecord | null,
  sourceArtifact: BackupArtifactRecord | null,
  targetClientId: string,
): RestoreArchiveTransferOption[] {
  if (!sourceBackup || !sourceArtifact || !targetClientId) {
    return [];
  }
  const artifactSha = sourceArtifact.sha256_hex.toLowerCase();
  return transfers
    .filter(
      (transfer) =>
        transfer.client_id === targetClientId &&
        transfer.direction === "upload" &&
        transfer.status === "completed" &&
        transfer.path.startsWith("/") &&
        transfer.size_bytes === sourceArtifact.size_bytes &&
        transfer.sha256_hex?.toLowerCase() === artifactSha,
    )
    .sort((left, right) => right.observed_at.localeCompare(left.observed_at))
    .map((transfer) => ({
      key: restoreArchiveTransferKeyForRecord(transfer),
      observedAt: transfer.observed_at,
      path: transfer.path,
      sessionId: transfer.session_id,
      sha256Hex: transfer.sha256_hex ?? "",
      sizeBytes: transfer.size_bytes ?? 0,
    }));
}

function activeRestoreArchiveKey(
  requestedKey: string,
  options: RestoreArchiveTransferOption[],
): string {
  if (options.some((option) => option.key === requestedKey)) {
    return requestedKey;
  }
  return options.length === 1 ? options[0].key : "";
}

function restoreArchiveTransferKeyForRecord(
  transfer: FileTransferSessionRecord,
): string {
  return `${transfer.client_id}:${transfer.session_id}`;
}

function restoreArchiveEmptyMessage(
  sourceBackupId: string,
  targetClientId: string,
  sourceBackup: BackupRequestRecord | null,
  sourceArtifact: BackupArtifactRecord | null,
): string {
  if (!sourceBackupId) {
    return "Select a source backup request first";
  }
  if (!targetClientId) {
    return "Select a restore target first";
  }
  if (!sourceBackup?.artifact_id) {
    return "Selected backup has no artifact record yet";
  }
  if (!sourceArtifact) {
    return "Selected backup artifact metadata is unavailable";
  }
  return "No completed upload on the target matches this backup artifact";
}

function generatedRestoreDestinationRoot(
  sourceBackupId: string,
  targetClientId: string,
): string {
  if (!sourceBackupId || !targetClientId) {
    return "";
  }
  return `/var/lib/vpsman/restores/${safeRestorePathSegment(
    sourceBackupId,
  )}/${safeRestorePathSegment(targetClientId)}`;
}

function safeRestorePathSegment(value: string): string {
  return value.replace(/[^A-Za-z0-9._-]+/g, "_").slice(0, 120) || "unknown";
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
  const partial = result.policies.some((policy) => policy.status === "partial_error");
  const action = result.dry_run ? "previewed" : partial ? "partially applied" : "applied";
  return `Policy prune ${action} ${totals.pruned || totals.matched} artifact${
    totals.pruned === 1 || (!totals.pruned && totals.matched === 1) ? "" : "s"
  }`;
}
