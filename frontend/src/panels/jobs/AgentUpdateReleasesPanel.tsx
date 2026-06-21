import { useEffect, useState } from "react";
import { PackageCheck } from "lucide-react";
import { ConfirmationPrompt } from "../../components/ConfirmationPrompt";
import { CrudPager } from "../../components/CrudPager";
import {
  DEFAULT_UPDATE_VERSION_URL,
  type JobDispatchPresetInput,
} from "../../jobDispatchPreset";
import { agentUpdateReleaseStatusBadgeClass } from "../../jobStatusPresentation";
import type {
  AgentUpdateReleaseRecord,
  CreateAgentUpdateReleaseRequest,
  JsonValue,
  SuiteConfigResponse,
} from "../../types";
import { formatTime, runPanelAction, shortHash } from "../../utils";

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
  loading,
  onCreateAgentUpdateRelease,
  onOpenDispatchPreset,
  onRefresh,
  releases,
  suiteConfig,
  suiteConfigError,
  suiteConfigLoading,
}: {
  loading: boolean;
  onCreateAgentUpdateRelease: (request: CreateAgentUpdateReleaseRequest) => Promise<AgentUpdateReleaseRecord>;
  onOpenDispatchPreset: (preset: JobDispatchPresetInput) => void;
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
  const policy = registeredUpdatePolicy(suiteConfig, suiteConfigError, suiteConfigLoading);

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
            onClick={() =>
              onOpenDispatchPreset({
                mode: "agent_update_activate",
                selectorExpression: "",
                updateActivationSha256Hex: latestRelease?.artifact_sha256_hex ?? "",
                updateRestartAgent: true,
                timeoutSecs: 60,
              })
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
                timeoutSecs: 60,
              })
            }
            type="button"
          >
            Rollback
          </button>
        </div>
      </div>
      <div className="releaseRecordForm">
        <div className="operationNote compactOperation">
          <PackageCheck size={18} />
          <div>
            <strong>External release metadata</strong>
            <span>Register HTTPS artifact hashes for strict API-side admission control; update jobs are dispatched from the command composer.</span>
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
        detail="Records the reviewed HTTPS artifact hashes for update admission."
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
      <CrudPager
        fields={[
          { label: "Release", value: (release) => `${release.name} ${release.version} ${release.channel}` },
          { label: "Status", value: (release) => release.status },
          { label: "Artifact", value: (release) => release.artifact_sha256_hex },
          { label: "Rollback", value: (release) => release.rollback_artifact_sha256_hex },
          { label: "URL hash", value: (release) => release.artifact_url_sha256_hex ?? "" },
        ]}
        itemLabel="releases"
        items={releases}
        pageSize={8}
        title="Release records"
        empty={
          <div className="emptyState">
            <PackageCheck size={22} />
            <strong>No release metadata</strong>
            <span>Record an external HTTPS artifact before enforcing registered updates.</span>
          </div>
        }
      >
        {(releaseRows) => (
          <div className="table historyTable">
            <div className="historyRow heading releaseGrid">
              <span>Release</span>
              <span>Status</span>
              <span>Artifact</span>
              <span>Rollback</span>
              <span>URL hash</span>
              <span>Created</span>
            </div>
            {releaseRows.map((release) => (
              <div className="historyRow releaseGrid" key={release.id}>
                <span className="historyPrimary">
                  <strong>{release.name}</strong>
                  <small>
                    {release.version} / {release.channel}
                  </small>
                </span>
                <span className={`status ${agentUpdateReleaseStatusBadgeClass(release.status)}`}>{release.status}</span>
                <span className="monoValue">{shortHash(release.artifact_sha256_hex)}</span>
                <span className="monoValue">
                  {release.rollback_artifact_sha256_hex ? shortHash(release.rollback_artifact_sha256_hex) : "none"}
                </span>
                <span className="monoValue">
                  {release.artifact_url_sha256_hex ? shortHash(release.artifact_url_sha256_hex) : "not stored"}
                  {release.rollback_artifact_url_sha256_hex && <small>{shortHash(release.rollback_artifact_url_sha256_hex)}</small>}
                </span>
                <span>{formatTime(release.created_at)}</span>
              </div>
            ))}
          </div>
        )}
      </CrudPager>
    </div>
  );
}

function registeredUpdatePolicy(
  suiteConfig: SuiteConfigResponse | null,
  suiteConfigError: string | null,
  suiteConfigLoading: boolean,
): { detail: string; label: string } {
  if (suiteConfigLoading) {
    return {
      label: "Registered-update policy loading",
      detail: "Loading suite config before showing whether direct manual updates require a registry entry.",
    };
  }
  if (suiteConfigError) {
    return {
      label: "Registered-update policy unavailable",
      detail: suiteConfigError,
    };
  }
  const enforced = readBooleanPath(suiteConfig?.redacted ?? null, ["api", "require_registered_agent_updates"]);
  if (enforced === true) {
    return {
      label: "Registered-update policy enforced",
      detail: "Manual update jobs are accepted only when the requested artifact SHA-256 exists in this registry.",
    };
  }
  if (enforced === false) {
    return {
      label: "Registered-update policy not enforced",
      detail: "This registry is optional audit metadata. Manifest-based update checks and direct manual updates can still be dispatched.",
    };
  }
  return {
    label: "Registered-update policy unknown",
    detail: "Open System config to confirm whether manual update jobs require registered artifact hashes.",
  };
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
