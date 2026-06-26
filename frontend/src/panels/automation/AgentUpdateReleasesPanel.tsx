import { useEffect, useMemo, useState } from "react";
import { ExternalLink, PackageCheck } from "lucide-react";
import { ConfirmationPrompt } from "../../components/ConfirmationPrompt";
import {
  ConsoleDataGrid,
  type ConsoleDataGridColumn,
} from "../../components/ConsoleDataGrid";
import {
  DEFAULT_UPDATE_VERSION_URL,
  type JobDispatchPresetInput,
} from "../../jobDispatchPreset";
import { agentUpdateReleaseStatusBadgeClass } from "../../jobStatusPresentation";
import type {
  AgentView,
  AgentUpdateReleaseRecord,
  CreateAgentUpdateReleaseRequest,
  JsonValue,
  JobHistoryRecord,
  SuiteConfigResponse,
} from "../../types";
import { formatTime, runPanelAction, shortHash } from "../../utils";

const agentUpdateCommandTypes = new Set([
  "agent_update",
  "agent_update_activate",
  "agent_update_check",
  "agent_update_rollback",
]);

function parseOptionalPositiveInteger(value: string, label: string): number | null {
  const trimmed = value.trim();
  if (!trimmed) return null;
  const parsed = Number(trimmed);
  if (!Number.isSafeInteger(parsed) || parsed < 1) {
    throw new Error(`${label} must be a positive integer`);
  }
  return parsed;
}

export function AgentUpdateReleasesPanel({
  agents,
  jobs,
  loading,
  onCreateAgentUpdateRelease,
  onOpenDispatchPreset,
  onOpenJobDetails,
  onOpenJobHistory,
  onRefresh,
  releases,
  suiteConfig,
  suiteConfigError,
  suiteConfigLoading,
}: {
  agents: AgentView[];
  jobs: JobHistoryRecord[];
  loading: boolean;
  onCreateAgentUpdateRelease: (request: CreateAgentUpdateReleaseRequest) => Promise<AgentUpdateReleaseRecord>;
  onOpenDispatchPreset: (preset: JobDispatchPresetInput) => void;
  onOpenJobDetails?: (jobId: string) => void;
  onOpenJobHistory: () => void;
  onRefresh: () => void;
  releases: AgentUpdateReleaseRecord[];
  suiteConfig: SuiteConfigResponse | null;
  suiteConfigError: string | null;
  suiteConfigLoading: boolean;
}) {
  const [releaseName, setReleaseName] = useState("vpsman-agent");
  const [releaseVersion, setReleaseVersion] = useState("");
  const [releaseChannel, setReleaseChannel] = useState("stable");
  const [releaseArtifactUrl, setReleaseArtifactUrl] = useState("");
  const [releaseSha256Hex, setReleaseSha256Hex] = useState("");
  const [releaseSizeBytes, setReleaseSizeBytes] = useState("");
  const [rollbackArtifactUrl, setRollbackArtifactUrl] = useState("");
  const [rollbackSha256Hex, setRollbackSha256Hex] = useState("");
  const [rollbackSizeBytes, setRollbackSizeBytes] = useState("");
  const [releaseNotes, setReleaseNotes] = useState("");
  const [releaseError, setReleaseError] = useState<string | null>(null);
  const [releasePending, setReleasePending] = useState(false);
  const [releaseSnapshot, setReleaseSnapshot] =
    useState<CreateAgentUpdateReleaseRequest | null>(null);
  const latestRelease = releases[0] ?? null;
  const latestArtifactSha256Hex = latestRelease?.artifact_sha256_hex ?? "";
  const policy = registeredUpdatePolicy(suiteConfig, suiteConfigError, suiteConfigLoading);
  const fleetVersionPosture = useMemo(() => buildFleetVersionPosture(agents), [agents]);
  const updateJobs = useMemo(
    () => jobs.filter((job) => agentUpdateCommandTypes.has(job.command_type)),
    [jobs],
  );
  const recentUpdateJob = updateJobs[0] ?? null;
  const rollbackDetail = latestRelease?.rollback_artifact_sha256_hex
    ? `Latest rollback hash ${shortHash(latestRelease.rollback_artifact_sha256_hex)} is available for dispatch review.`
    : latestRelease
      ? "The latest registered release has no rollback hash. Rollback dispatch can still be opened, but no registry hash will be prefilled."
      : "Record a release before using the registry as rollback evidence.";
  const releasePostureItems = [
    {
      detail: latestRelease
        ? `Latest ${latestRelease.name} ${latestRelease.version} / ${latestRelease.channel}, ${latestRelease.status}.`
        : "No external artifact metadata has been recorded yet.",
      label: "Registered releases",
      value: releases.length ? `${releases.length} registered` : "None",
    },
    {
      detail: fleetVersionPosture.detail,
      label: "Current fleet versions",
      value: fleetVersionPosture.value,
    },
    {
      detail: latestArtifactSha256Hex
        ? "Use Check latest on a canary selector, inspect job evidence, then activate the staged hash for the reviewed rollout scope."
        : "Record or check a release artifact before a staged rollout can be activated from this page.",
      label: "Canary / staging",
      value: latestArtifactSha256Hex ? "Manual scoped rollout" : "No staged hash",
    },
    {
      detail: policy.detail,
      label: "Rollout policy",
      value: policy.value,
    },
    {
      detail: recentUpdateJob
        ? `Last ${displayCommandType(recentUpdateJob.command_type)} job targeted ${recentUpdateJob.target_count} VPS${recentUpdateJob.target_count === 1 ? "" : "s"} at ${formatTime(recentUpdateJob.created_at)}.`
        : "Run Check latest to produce per-target update evidence before activation.",
      label: "Health checks",
      value: recentUpdateJob ? recentUpdateJob.status : "No update jobs",
    },
    {
      detail: rollbackDetail,
      label: "Rollback",
      value: latestRelease?.rollback_artifact_sha256_hex
        ? "Hash registered"
        : latestRelease
          ? "Dispatch available"
          : "No release",
    },
  ];
  const releaseColumns = useMemo<ConsoleDataGridColumn<AgentUpdateReleaseRecord>[]>(
    () => [
      {
        cell: (release) => (
          <span className="historyPrimary">
            <strong>{release.name}</strong>
            <small>
              {release.version} / {release.channel}
            </small>
          </span>
        ),
        header: "Release",
        id: "release",
        searchValue: (release) => `${release.name} ${release.version} ${release.channel}`,
        sortValue: (release) => `${release.name}:${release.version}`,
      },
      {
        cell: (release) => (
          <span className={`status ${agentUpdateReleaseStatusBadgeClass(release.status)}`}>
            {release.status}
          </span>
        ),
        header: "Status",
        id: "status",
        searchValue: (release) => release.status,
        sortValue: (release) => release.status,
      },
      {
        cell: (release) => (
          <span className="monoValue" title={release.artifact_sha256_hex}>
            {shortHash(release.artifact_sha256_hex)}
          </span>
        ),
        header: "Artifact",
        id: "artifact",
        searchValue: (release) => release.artifact_sha256_hex,
        sortValue: (release) => release.artifact_sha256_hex,
      },
      {
        cell: (release) => (
          <span
            className="monoValue"
            title={release.rollback_artifact_sha256_hex ?? undefined}
          >
            {release.rollback_artifact_sha256_hex
              ? shortHash(release.rollback_artifact_sha256_hex)
              : "none"}
          </span>
        ),
        header: "Rollback",
        id: "rollback",
        searchValue: (release) => release.rollback_artifact_sha256_hex ?? "",
        sortValue: (release) => release.rollback_artifact_sha256_hex ?? "",
      },
      {
        cell: (release) => (
          <span className="monoValue" title={release.artifact_url_sha256_hex ?? undefined}>
            {release.artifact_url_sha256_hex
              ? shortHash(release.artifact_url_sha256_hex)
              : "not stored"}
            {release.rollback_artifact_url_sha256_hex && (
              <small>{shortHash(release.rollback_artifact_url_sha256_hex)}</small>
            )}
          </span>
        ),
        header: "URL hash",
        id: "urlHash",
        searchValue: (release) =>
          `${release.artifact_url_sha256_hex ?? ""} ${release.rollback_artifact_url_sha256_hex ?? ""}`,
        sortValue: (release) => release.artifact_url_sha256_hex ?? "",
      },
      {
        cell: (release) => formatTime(release.created_at),
        header: "Created",
        id: "created",
        searchValue: (release) => formatTime(release.created_at),
        sortValue: (release) => release.created_at,
      },
    ],
    [],
  );

  useEffect(() => {
    setReleaseSnapshot(null);
  }, [
    releaseName,
    releaseVersion,
    releaseChannel,
    releaseArtifactUrl,
    releaseSha256Hex,
    releaseSizeBytes,
    rollbackArtifactUrl,
    rollbackSha256Hex,
    rollbackSizeBytes,
    releaseNotes,
  ]);

  function releaseRequest(): CreateAgentUpdateReleaseRequest {
    const artifactUrl = releaseArtifactUrl.trim();
    const sha256Hex = releaseSha256Hex.trim().toLowerCase();
    if (!artifactUrl.startsWith("https://")) {
      throw new Error("Artifact URL must use https://");
    }
    if (!/^[0-9a-f]{64}$/.test(sha256Hex)) {
      throw new Error("Artifact SHA-256 must be 64 hex characters");
    }
    const rollbackUrl = rollbackArtifactUrl.trim();
    const rollbackSha = rollbackSha256Hex.trim().toLowerCase();
    const hasRollback = Boolean(rollbackUrl || rollbackSha || rollbackSizeBytes.trim());
    if (hasRollback) {
      if (!rollbackUrl.startsWith("https://")) {
        throw new Error("Rollback artifact URL must use https://");
      }
      if (!/^[0-9a-f]{64}$/.test(rollbackSha)) {
        throw new Error("Rollback SHA-256 must be 64 hex characters");
      }
    }
    return {
      name: releaseName.trim(),
      version: releaseVersion.trim(),
      channel: releaseChannel.trim() || "stable",
      artifact_url: artifactUrl,
      artifact_sha256_hex: sha256Hex,
      rollback_artifact_sha256_hex: hasRollback ? rollbackSha : null,
      rollback_artifact_url: hasRollback ? rollbackUrl : null,
      rollback_size_bytes: hasRollback ? parseOptionalPositiveInteger(rollbackSizeBytes, "Rollback size") : null,
      size_bytes: parseOptionalPositiveInteger(releaseSizeBytes, "Size"),
      notes: releaseNotes.trim() || null,
      confirmed: true,
    };
  }

  async function reviewAgentUpdateRelease() {
    await runPanelAction(setReleasePending, setReleaseError, async () => {
      setReleaseSnapshot(releaseRequest());
    });
  }

  async function recordAgentUpdateRelease() {
    const snapshot = releaseSnapshot;
    if (!snapshot) {
      setReleaseError("Review release metadata before recording");
      return;
    }
    await runPanelAction(setReleasePending, setReleaseError, async () => {
      await onCreateAgentUpdateRelease(snapshot);
      setReleaseSnapshot(null);
      clearReleaseInputs();
    });
  }

  function clearReleaseInputs() {
    setReleaseVersion("");
    setReleaseArtifactUrl("");
    setReleaseSha256Hex("");
    setReleaseSizeBytes("");
    setRollbackArtifactUrl("");
    setRollbackSha256Hex("");
    setRollbackSizeBytes("");
    setReleaseNotes("");
  }

  return (
    <div className="fleetPanel agentReleasesPanel">
      <div className="sectionHeader">
        <div>
          <h2>Agent update registry</h2>
          <span>{releases.length} registered external artifact{releases.length === 1 ? "" : "s"}</span>
        </div>
        <div className="sectionActions">
          <button
            className="primaryAction compactAction"
            onClick={() =>
              onOpenDispatchPreset({
                mode: "agent_update_check",
                selectorExpression: "",
                updateCheckActivate: true,
                updateCheckRestartAgent: true,
                updateCheckVersionUrl: DEFAULT_UPDATE_VERSION_URL,
              })
            }
            type="button"
          >
            Check latest GitHub update
          </button>
          <button className="secondaryAction compactAction" disabled={loading} onClick={onRefresh} type="button">
            Refresh
          </button>
        </div>
      </div>
      <div className="releaseWorkflowBar">
        <div>
          <strong>{policy.label}</strong>
          <span>{policy.detail}</span>
        </div>
        <div className="releaseQuickActions" aria-label="Agent update dispatch shortcuts">
          <button
            className="secondaryAction compactAction"
            disabled={!latestArtifactSha256Hex}
            onClick={() =>
              onOpenDispatchPreset({
                mode: "agent_update_activate",
                selectorExpression: "",
                updateActivationSha256Hex: latestArtifactSha256Hex,
                updateRestartAgent: true,
                maxTimeoutSecs: 60,
              })
            }
            title={
              latestArtifactSha256Hex
                ? "Open dispatch with the latest registered artifact hash."
                : "Record a release artifact before using this shortcut."
            }
            type="button"
          >
            Activate staged
          </button>
          <button
            className="secondaryAction compactAction"
            onClick={() =>
              onOpenDispatchPreset({
                mode: "agent_update_rollback",
                selectorExpression: "",
                updateRollbackSha256Hex: latestRelease?.rollback_artifact_sha256_hex ?? "",
                maxTimeoutSecs: 60,
              })
            }
            type="button"
          >
            Rollback
          </button>
        </div>
      </div>
      <section className="releasePosture" aria-label="Agent update rollout posture">
        <div className="releasePostureHeader">
          <div>
            <strong>Rollout posture</strong>
            <span>Automation owns release registry, rollout planning, update evidence, and rollback readiness.</span>
          </div>
          <button className="secondaryAction compactAction" onClick={onOpenJobHistory} type="button">
            Open update jobs
          </button>
          {recentUpdateJob && onOpenJobDetails ? (
            <button
              className="secondaryAction compactAction"
              onClick={() => onOpenJobDetails(recentUpdateJob.id)}
              type="button"
            >
              <ExternalLink size={14} />
              <span>Open last update job</span>
            </button>
          ) : null}
        </div>
        <div className="releasePostureGrid">
          {releasePostureItems.map((item) => (
            <div className="releasePostureItem" key={item.label}>
              <span>{item.label}</span>
              <strong>{item.value}</strong>
              <small>{item.detail}</small>
            </div>
          ))}
        </div>
      </section>
      <div className="releaseRecordForm">
        <div className="operationNote compactOperation">
          <PackageCheck size={18} />
          <div>
            <strong>External release metadata</strong>
            <span>Register HTTPS artifact hashes for strict update approval; check jobs verify the resolved agent artifact hash before staging.</span>
          </div>
        </div>

        <div className="releaseFormSection releaseIdentitySection">
          <div className="releaseFormSectionHeader">
            <strong>Release identity</strong>
          </div>
          <label>
            <span>Name</span>
            <input aria-label="Release name" onChange={(event) => setReleaseName(event.target.value)} value={releaseName} />
          </label>
          <label>
            <span>Version</span>
            <input aria-label="Release version" onChange={(event) => setReleaseVersion(event.target.value)} value={releaseVersion} />
          </label>
          <label>
            <span>Channel</span>
            <input aria-label="Release channel" onChange={(event) => setReleaseChannel(event.target.value)} value={releaseChannel} />
          </label>
          <label>
            <span>Notes</span>
            <input aria-label="Release notes" onChange={(event) => setReleaseNotes(event.target.value)} value={releaseNotes} />
          </label>
        </div>

        <div className="releaseFormSection releaseArtifactSection">
          <div className="releaseFormSectionHeader">
            <strong>Primary artifact</strong>
          </div>
          <label className="wideField">
            <span>Artifact URL</span>
            <input
              aria-label="Release artifact URL"
              onChange={(event) => setReleaseArtifactUrl(event.target.value)}
              placeholder="https://github.com/owner/repo/releases/download/tag/vpsman-agent-linux-x86_64-musl"
              value={releaseArtifactUrl}
            />
          </label>
          <label>
            <span>SHA-256</span>
            <input aria-label="Release SHA-256" onChange={(event) => setReleaseSha256Hex(event.target.value)} value={releaseSha256Hex} />
          </label>
          <label>
            <span>Size bytes</span>
            <input
              aria-label="Release size bytes"
              min={1}
              onChange={(event) => setReleaseSizeBytes(event.target.value)}
              type="number"
              value={releaseSizeBytes}
            />
          </label>
        </div>

        <div className="releaseFormSection releaseArtifactSection">
          <div className="releaseFormSectionHeader">
            <strong>Rollback artifact</strong>
          </div>
          <label className="wideField">
            <span>Rollback URL</span>
            <input
              aria-label="Rollback artifact URL"
              onChange={(event) => setRollbackArtifactUrl(event.target.value)}
              placeholder="https://github.com/owner/repo/releases/download/tag/vpsman-agent-previous"
              value={rollbackArtifactUrl}
            />
          </label>
          <label>
            <span>Rollback SHA-256</span>
            <input aria-label="Rollback SHA-256" onChange={(event) => setRollbackSha256Hex(event.target.value)} value={rollbackSha256Hex} />
          </label>
          <label>
            <span>Rollback size</span>
            <input
              aria-label="Rollback size bytes"
              min={1}
              onChange={(event) => setRollbackSizeBytes(event.target.value)}
              type="number"
              value={rollbackSizeBytes}
            />
          </label>
        </div>

        <div className="releaseFormActions">
          <button className="primaryAction" disabled={releasePending} onClick={() => void reviewAgentUpdateRelease()} type="button">
            Review release
          </button>
        </div>
        {releaseError && <span className="inlineError">{releaseError}</span>}
      </div>
      <ConfirmationPrompt
        confirmLabel="Record release"
        detail="Records the reviewed HTTPS artifact hashes for update approval."
        items={[
          { label: "Release", value: releaseSnapshot ? `${releaseSnapshot.name} ${releaseSnapshot.version}` : "-" },
          { label: "Channel", value: releaseSnapshot?.channel ?? "-" },
          {
            label: "Artifact",
            title: releaseSnapshot?.artifact_sha256_hex,
            value: releaseSnapshot ? shortHash(releaseSnapshot.artifact_sha256_hex) : "-",
          },
          {
            label: "Rollback",
            title: releaseSnapshot?.rollback_artifact_sha256_hex ?? undefined,
            value: releaseSnapshot?.rollback_artifact_sha256_hex
              ? shortHash(releaseSnapshot.rollback_artifact_sha256_hex)
              : "none",
          },
        ]}
        onCancel={() => setReleaseSnapshot(null)}
        onConfirm={() => void recordAgentUpdateRelease()}
        open={releaseSnapshot !== null}
        pending={releasePending}
        title="Confirm agent update release"
      />
      <ConsoleDataGrid
        columns={releaseColumns}
        defaultPageSize={8}
        expandOnRowClick
        getRowId={(release) => release.id}
        itemLabel="releases"
        empty={
          <div className="emptyState">
            <PackageCheck size={22} />
            <strong>No release metadata</strong>
            <span>Record an external HTTPS artifact before enforcing registered updates.</span>
          </div>
        }
        renderExpandedRow={(release) => (
          <div className="consoleInlineDetailGrid">
            <span>Release ID</span>
            <strong>{release.id}</strong>
            <span>Artifact SHA-256</span>
            <strong>{release.artifact_sha256_hex}</strong>
            <span>Artifact URL hash</span>
            <strong>{release.artifact_url_sha256_hex ?? "Not stored"}</strong>
            <span>Rollback SHA-256</span>
            <strong>{release.rollback_artifact_sha256_hex ?? "None"}</strong>
            <span>Rollback URL hash</span>
            <strong>{release.rollback_artifact_url_sha256_hex ?? "None"}</strong>
            <span>Created</span>
            <strong>{formatTime(release.created_at)}</strong>
          </div>
        )}
        rows={releases}
        searchPlaceholder="Search releases"
        selectable={false}
        storageKey="vpsman.automation.agentUpdateReleases"
        title="Release records"
      />
    </div>
  );
}

function registeredUpdatePolicy(
  suiteConfig: SuiteConfigResponse | null,
  suiteConfigError: string | null,
  suiteConfigLoading: boolean,
): { detail: string; label: string; value: string } {
  if (suiteConfigLoading) {
    return {
      label: "Registered-update policy loading",
      detail: "Loading suite config before showing whether direct manual updates require a registry entry.",
      value: "Loading",
    };
  }
  if (suiteConfigError) {
    return {
      label: "Registered-update policy unavailable",
      detail: suiteConfigError,
      value: "Unavailable",
    };
  }
  const enforced = readBooleanPath(suiteConfig?.redacted ?? null, ["api", "require_registered_agent_updates"]);
  if (enforced === true) {
    return {
      label: "Registered-update policy enforced",
      detail: "Manual update jobs are accepted only when the requested artifact SHA-256 exists in this registry.",
      value: "Enforced",
    };
  }
  if (enforced === false) {
    return {
      label: "Registered-update policy not enforced",
      detail: "This registry is optional audit metadata. Manifest-based update checks and direct manual updates can still be dispatched.",
      value: "Advisory",
    };
  }
  return {
    label: "Registered-update policy unknown",
    detail: "Open Suite config to confirm whether manual update jobs require registered artifact hashes.",
    value: "Unknown",
  };
}

function buildFleetVersionPosture(agents: AgentView[]): { detail: string; value: string } {
  if (!agents.length) {
    return {
      detail: "No VPS inventory is loaded, so current agent version posture cannot be compared.",
      value: "No VPS loaded",
    };
  }
  const buildCounts = new Map<number, number>();
  for (const agent of agents) {
    if (Number.isSafeInteger(agent.internal_build_number)) {
      const build = agent.internal_build_number as number;
      buildCounts.set(build, (buildCounts.get(build) ?? 0) + 1);
    }
  }
  if (!buildCounts.size) {
    return {
      detail: "Loaded agents do not expose semantic agent version or internal build telemetry.",
      value: "Version telemetry unavailable",
    };
  }
  const knownCount = Array.from(buildCounts.values()).reduce((total, count) => total + count, 0);
  const distribution = Array.from(buildCounts.entries())
    .sort(([left], [right]) => right - left)
    .map(([build, count]) => `build ${build}: ${count}`)
    .join(", ");
  const unknownCount = agents.length - knownCount;
  return {
    detail: unknownCount
      ? `${distribution}; ${unknownCount} VPS${unknownCount === 1 ? "" : "s"} missing build telemetry.`
      : distribution,
    value: `${knownCount}/${agents.length} with build`,
  };
}

function displayCommandType(commandType: string): string {
  return commandType.replace(/_/g, " ");
}

function readBooleanPath(value: JsonValue | null, path: string[]): boolean | null {
  let current: JsonValue | undefined | null = value;
  for (const key of path) {
    if (!isJsonRecord(current)) {
      return null;
    }
    current = current[key];
  }
  return typeof current === "boolean" ? current : null;
}

function isJsonRecord(value: JsonValue | undefined | null): value is Record<string, JsonValue> {
  return Boolean(value) && typeof value === "object" && !Array.isArray(value);
}
