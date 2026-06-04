import { useState } from "react";
import { PackageCheck } from "lucide-react";
import { CrudPager } from "../../components/CrudPager";
import type {
  AgentUpdateReleaseRecord,
  CreateAgentUpdateReleaseRequest,
  UploadAgentUpdateArtifactRequest,
} from "../../types";
import { formatTime, runPanelAction, shortHash, statusClass } from "../../utils";

async function fileToBase64(file: File): Promise<string> {
  const bytes = new Uint8Array(await file.arrayBuffer());
  let binary = "";
  const chunkSize = 0x8000;
  for (let index = 0; index < bytes.length; index += chunkSize) {
    binary += String.fromCharCode(...bytes.subarray(index, index + chunkSize));
  }
  return window.btoa(binary);
}

export function AgentUpdateReleasesPanel({
  loading,
  onCreateAgentUpdateRelease,
  onRefresh,
  onUploadAgentUpdateArtifact,
  releases,
}: {
  loading: boolean;
  onCreateAgentUpdateRelease: (request: CreateAgentUpdateReleaseRequest) => Promise<AgentUpdateReleaseRecord>;
  onRefresh: () => void;
  onUploadAgentUpdateArtifact: (request: UploadAgentUpdateArtifactRequest) => Promise<AgentUpdateReleaseRecord>;
  releases: AgentUpdateReleaseRecord[];
}) {
  const [releaseName, setReleaseName] = useState("vpsman-agent");
  const [releaseVersion, setReleaseVersion] = useState("");
  const [releaseChannel, setReleaseChannel] = useState("stable");
  const [releaseArtifactUrl, setReleaseArtifactUrl] = useState("");
  const [releaseSha256Hex, setReleaseSha256Hex] = useState("");
  const [releaseSignatureHex, setReleaseSignatureHex] = useState("");
  const [releaseSigningKeyHex, setReleaseSigningKeyHex] = useState("");
  const [releaseSizeBytes, setReleaseSizeBytes] = useState("");
  const [rollbackArtifactUrl, setRollbackArtifactUrl] = useState("");
  const [rollbackSha256Hex, setRollbackSha256Hex] = useState("");
  const [rollbackSignatureHex, setRollbackSignatureHex] = useState("");
  const [rollbackSigningKeyHex, setRollbackSigningKeyHex] = useState("");
  const [rollbackSizeBytes, setRollbackSizeBytes] = useState("");
  const [releaseNotes, setReleaseNotes] = useState("");
  const [releaseUploadFile, setReleaseUploadFile] = useState<File | null>(null);
  const [rollbackUploadFile, setRollbackUploadFile] = useState<File | null>(null);
  const [releaseError, setReleaseError] = useState<string | null>(null);
  const [releasePending, setReleasePending] = useState(false);

  async function recordAgentUpdateRelease() {
    await runPanelAction(setReleasePending, setReleaseError, async () => {
      const sha256Hex = releaseSha256Hex.trim().toLowerCase();
      const signatureHex = releaseSignatureHex.trim().toLowerCase();
      const signingKeyHex = releaseSigningKeyHex.trim().toLowerCase();
      if (!releaseArtifactUrl.trim().startsWith("https://")) {
        throw new Error("Artifact URL must use https://");
      }
      if (!/^[0-9a-f]{64}$/.test(sha256Hex)) {
        throw new Error("Artifact SHA-256 must be 64 hex characters");
      }
      if (!/^[0-9a-f]{128}$/.test(signatureHex)) {
        throw new Error("Signature must be 128 hex characters");
      }
      if (!/^[0-9a-f]{64}$/.test(signingKeyHex)) {
        throw new Error("Signing key must be 64 hex characters");
      }
      const rollbackSha = rollbackSha256Hex.trim().toLowerCase();
      const rollbackSignature = rollbackSignatureHex.trim().toLowerCase();
      const rollbackSigningKey = rollbackSigningKeyHex.trim().toLowerCase();
      const hasRollback = Boolean(
        rollbackArtifactUrl.trim() || rollbackSha || rollbackSignature || rollbackSigningKey || rollbackSizeBytes.trim(),
      );
      if (hasRollback) {
        if (!rollbackArtifactUrl.trim().startsWith("https://")) {
          throw new Error("Rollback artifact URL must use https://");
        }
        if (!/^[0-9a-f]{64}$/.test(rollbackSha)) {
          throw new Error("Rollback SHA-256 must be 64 hex characters");
        }
        if (!/^[0-9a-f]{128}$/.test(rollbackSignature)) {
          throw new Error("Rollback signature must be 128 hex characters");
        }
        if (!/^[0-9a-f]{64}$/.test(rollbackSigningKey)) {
          throw new Error("Rollback signing key must be 64 hex characters");
        }
      }
      await onCreateAgentUpdateRelease({
        name: releaseName.trim(),
        version: releaseVersion.trim(),
        channel: releaseChannel.trim() || "stable",
        artifact_url: releaseArtifactUrl.trim(),
        artifact_sha256_hex: sha256Hex,
        artifact_signature_hex: signatureHex,
        artifact_signing_key_hex: signingKeyHex,
        rollback_artifact_sha256_hex: hasRollback ? rollbackSha : null,
        rollback_artifact_signature_hex: hasRollback ? rollbackSignature : null,
        rollback_artifact_signing_key_hex: hasRollback ? rollbackSigningKey : null,
        rollback_artifact_url: hasRollback ? rollbackArtifactUrl.trim() : null,
        rollback_size_bytes: hasRollback && rollbackSizeBytes.trim() ? Number(rollbackSizeBytes.trim()) : null,
        size_bytes: releaseSizeBytes.trim() ? Number(releaseSizeBytes.trim()) : null,
        notes: releaseNotes.trim() || null,
        confirmed: true,
      });
      clearReleaseInputs();
    });
  }

  async function uploadAgentUpdateArtifact() {
    await runPanelAction(setReleasePending, setReleaseError, async () => {
      if (!releaseUploadFile) {
        throw new Error("Select an artifact file");
      }
      const signatureHex = releaseSignatureHex.trim().toLowerCase();
      const signingKeyHex = releaseSigningKeyHex.trim().toLowerCase();
      if (!/^[0-9a-f]{128}$/.test(signatureHex)) {
        throw new Error("Signature must be 128 hex characters");
      }
      if (!/^[0-9a-f]{64}$/.test(signingKeyHex)) {
        throw new Error("Signing key must be 64 hex characters");
      }
      const rollbackSignature = rollbackSignatureHex.trim().toLowerCase();
      const rollbackSigningKey = rollbackSigningKeyHex.trim().toLowerCase();
      if (rollbackUploadFile) {
        if (!/^[0-9a-f]{128}$/.test(rollbackSignature)) {
          throw new Error("Rollback signature must be 128 hex characters");
        }
        if (!/^[0-9a-f]{64}$/.test(rollbackSigningKey)) {
          throw new Error("Rollback signing key must be 64 hex characters");
        }
      }
      await onUploadAgentUpdateArtifact({
        name: releaseName.trim(),
        version: releaseVersion.trim(),
        channel: releaseChannel.trim() || "stable",
        artifact_base64: await fileToBase64(releaseUploadFile),
        artifact_signature_hex: signatureHex,
        artifact_signing_key_hex: signingKeyHex,
        rollback_artifact_base64: rollbackUploadFile ? await fileToBase64(rollbackUploadFile) : null,
        rollback_artifact_signature_hex: rollbackUploadFile ? rollbackSignature : null,
        rollback_artifact_signing_key_hex: rollbackUploadFile ? rollbackSigningKey : null,
        notes: releaseNotes.trim() || null,
        confirmed: true,
      });
      clearReleaseInputs();
      setReleaseUploadFile(null);
      setRollbackUploadFile(null);
    });
  }

  function clearReleaseInputs() {
    setReleaseVersion("");
    setReleaseArtifactUrl("");
    setReleaseSha256Hex("");
    setReleaseSignatureHex("");
    setReleaseSigningKeyHex("");
    setReleaseSizeBytes("");
    setRollbackArtifactUrl("");
    setRollbackSha256Hex("");
    setRollbackSignatureHex("");
    setRollbackSigningKeyHex("");
    setRollbackSizeBytes("");
    setReleaseNotes("");
  }

  return (
    <div className="fleetPanel">
      <div className="sectionHeader">
        <div>
          <h2>Agent update releases</h2>
          <span>{releases.length} signed metadata records</span>
        </div>
        <button className="secondaryAction" disabled={loading} onClick={onRefresh} type="button">
          Refresh
        </button>
      </div>
      <div className="releaseRecordForm">
        <div className="operationNote compactOperation">
          <PackageCheck size={18} />
          <div>
            <strong>Record signed release</strong>
            <span>Stores sanitized metadata only: hashes, channel, and signature evidence</span>
          </div>
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
        <label className="wideField">
          <span>Artifact URL</span>
          <input
            aria-label="Release artifact URL"
            onChange={(event) => setReleaseArtifactUrl(event.target.value)}
            placeholder="https://updates.example/vpsman-agent"
            value={releaseArtifactUrl}
          />
        </label>
        <label className="wideField">
          <span>SHA-256</span>
          <input aria-label="Release SHA-256" onChange={(event) => setReleaseSha256Hex(event.target.value)} value={releaseSha256Hex} />
        </label>
        <label className="wideField">
          <span>Signature</span>
          <input aria-label="Release signature" onChange={(event) => setReleaseSignatureHex(event.target.value)} value={releaseSignatureHex} />
        </label>
        <label className="wideField">
          <span>Signing key</span>
          <input aria-label="Release signing key" onChange={(event) => setReleaseSigningKeyHex(event.target.value)} value={releaseSigningKeyHex} />
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
        <label className="wideField">
          <span>Rollback URL</span>
          <input
            aria-label="Rollback artifact URL"
            onChange={(event) => setRollbackArtifactUrl(event.target.value)}
            placeholder="https://updates.example/vpsman-agent.previous"
            value={rollbackArtifactUrl}
          />
        </label>
        <label className="wideField">
          <span>Rollback SHA-256</span>
          <input aria-label="Rollback SHA-256" onChange={(event) => setRollbackSha256Hex(event.target.value)} value={rollbackSha256Hex} />
        </label>
        <label className="wideField">
          <span>Rollback signature</span>
          <input
            aria-label="Rollback signature"
            onChange={(event) => setRollbackSignatureHex(event.target.value)}
            value={rollbackSignatureHex}
          />
        </label>
        <label className="wideField">
          <span>Rollback signing key</span>
          <input
            aria-label="Rollback signing key"
            onChange={(event) => setRollbackSigningKeyHex(event.target.value)}
            value={rollbackSigningKeyHex}
          />
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
        <label>
          <span>Notes</span>
          <input aria-label="Release notes" onChange={(event) => setReleaseNotes(event.target.value)} value={releaseNotes} />
        </label>
        <label>
          <span>Artifact file</span>
          <input aria-label="Release artifact file" onChange={(event) => setReleaseUploadFile(event.target.files?.[0] ?? null)} type="file" />
        </label>
        <label>
          <span>Rollback file</span>
          <input aria-label="Rollback artifact file" onChange={(event) => setRollbackUploadFile(event.target.files?.[0] ?? null)} type="file" />
        </label>
        <button className="primaryAction" disabled={releasePending} onClick={recordAgentUpdateRelease} type="button">
          Record
        </button>
        <button className="secondaryAction" disabled={releasePending} onClick={uploadAgentUpdateArtifact} type="button">
          Upload
        </button>
        {releaseError && <span className="inlineError">{releaseError}</span>}
      </div>
      <CrudPager
        fields={[
          { label: "Release", value: (release) => `${release.name} ${release.version} ${release.channel}` },
          { label: "Status", value: (release) => release.status },
          { label: "Artifact", value: (release) => release.artifact_sha256_hex },
          { label: "Rollback", value: (release) => release.rollback_artifact_sha256_hex },
          { label: "Signature", value: (release) => release.artifact_signature_sha256_hex },
          { label: "Source", value: (release) => `${release.artifact_download_url ?? ""} ${release.artifact_download_path ?? ""}` },
        ]}
        itemLabel="releases"
        items={releases}
        pageSize={8}
        title="Release records"
        empty={
          <div className="emptyState">
            <PackageCheck size={22} />
            <strong>No release metadata</strong>
            <span>Publish signed metadata before enforcing registered agent updates.</span>
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
              <span>Signature</span>
              <span>Source</span>
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
                <span className={`status ${statusClass(release.status)}`}>{release.status}</span>
                <span className="monoValue">{shortHash(release.artifact_sha256_hex)}</span>
                <span className="monoValue">
                  {release.rollback_artifact_sha256_hex ? shortHash(release.rollback_artifact_sha256_hex) : "none"}
                </span>
                <span className="monoValue">{release.artifact_signature_sha256_hex ? shortHash(release.artifact_signature_sha256_hex) : "unsigned"}</span>
                <span className="monoValue">
                  {release.artifact_download_url ??
                    release.artifact_download_path ??
                    (release.artifact_url_sha256_hex ? shortHash(release.artifact_url_sha256_hex) : "not stored")}
                  {release.rollback_artifact_download_url && <small>{release.rollback_artifact_download_url}</small>}
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
