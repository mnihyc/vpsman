import { useState } from "react";
import { LockKeyhole, Save, ShieldCheck, Trash2 } from "lucide-react";
import { normalizeHex, type ProofMaterial } from "../proof";
import { clearProofVault, hasProofVault, loadProofVault, saveProofVault } from "../vault";
import { runPanelAction, shortHash } from "../utils";

type ProofVaultBoxProps = {
  labelPrefix?: string;
  lastPayloadHash: string | null;
  onProofMaterialChange: (material: ProofMaterial | null) => void;
  proofMaterial: ProofMaterial | null;
  clearVaultLabel?: string;
  lockProofLabel?: string;
  unlockLabel?: string;
  useProofLabel?: string;
};

export function ProofVaultBox({
  clearVaultLabel = "Clear vault",
  labelPrefix = "",
  lastPayloadHash,
  lockProofLabel = "Lock proof",
  onProofMaterialChange,
  proofMaterial,
  unlockLabel = "Unlock",
  useProofLabel = "Use proof",
}: ProofVaultBoxProps) {
  const [superPassword, setSuperPassword] = useState("");
  const [superSaltHex, setSuperSaltHex] = useState("");
  const [vaultPassphrase, setVaultPassphrase] = useState("");
  const [unlockPassphrase, setUnlockPassphrase] = useState("");
  const [saveToVault, setSaveToVault] = useState(false);
  const [vaultAvailable, setVaultAvailable] = useState(() => hasProofVault());
  const [actionError, setActionError] = useState<string | null>(null);
  const [pending, setPending] = useState(false);
  const proofStatus = proofMaterial ? "Proof unlocked" : vaultAvailable ? "Encrypted vault locked" : "Proof locked";
  const label = (value: string) => {
    if (!labelPrefix) {
      return value;
    }
    if (value === "Super password") {
      return `${labelPrefix} proof secret`;
    }
    if (value === "Super salt hex") {
      return `${labelPrefix} proof salt`;
    }
    return `${labelPrefix} proof ${value.toLowerCase()}`;
  };

  async function unlockVault() {
    await runPanelAction(setPending, setActionError, async () => {
      onProofMaterialChange(await loadProofVault(unlockPassphrase));
      setUnlockPassphrase("");
    });
  }

  async function activateEnteredProof() {
    await runPanelAction(setPending, setActionError, async () => {
      const material = {
        superPassword,
        superSaltHex: normalizeHex(superSaltHex),
      };
      if (saveToVault) {
        await saveProofVault(material, vaultPassphrase);
        setVaultAvailable(true);
        setVaultPassphrase("");
      }
      onProofMaterialChange(material);
      setSuperPassword("");
      setSuperSaltHex("");
    });
  }

  function lockProof() {
    onProofMaterialChange(null);
    setActionError(null);
  }

  function removeVault() {
    clearProofVault();
    setVaultAvailable(false);
    onProofMaterialChange(null);
    setActionError(null);
  }

  return (
    <div className="proofManager">
      <div className="proofStatus">
        <ShieldCheck size={18} />
        <div>
          <strong>{actionError ?? proofStatus}</strong>
          <span>{lastPayloadHash ? shortHash(lastPayloadHash) : "Envelope-only local proof"}</span>
        </div>
      </div>

      {proofMaterial ? (
        <button className="secondaryAction" onClick={lockProof} type="button">
          <LockKeyhole size={17} />
          {lockProofLabel}
        </button>
      ) : (
        <div className="proofForms">
          {vaultAvailable && (
            <div className="inlineProof">
              <input
                aria-label={label("Vault passphrase")}
                onChange={(event) => setUnlockPassphrase(event.target.value)}
                placeholder="vault passphrase"
                type="password"
                value={unlockPassphrase}
              />
              <button className="secondaryAction" disabled={pending || !unlockPassphrase} onClick={unlockVault} type="button">
                <LockKeyhole size={17} />
                {unlockLabel}
              </button>
            </div>
          )}
          <div className="proofFields">
            <input
              aria-label={label("Super password")}
              onChange={(event) => setSuperPassword(event.target.value)}
              placeholder="super password"
              type="password"
              value={superPassword}
            />
            <input
              aria-label={label("Super salt hex")}
              onChange={(event) => setSuperSaltHex(event.target.value)}
              placeholder="super salt hex"
              value={superSaltHex}
            />
          </div>
          <label className="checkLine">
            <input checked={saveToVault} onChange={(event) => setSaveToVault(event.target.checked)} type="checkbox" />
            <span>Save encrypted vault</span>
          </label>
          {saveToVault && (
            <input
              aria-label={label("New vault passphrase")}
              onChange={(event) => setVaultPassphrase(event.target.value)}
              placeholder="new vault passphrase"
              type="password"
              value={vaultPassphrase}
            />
          )}
          <button
            className="secondaryAction"
            disabled={pending || !superPassword || !superSaltHex || (saveToVault && !vaultPassphrase)}
            onClick={activateEnteredProof}
            type="button"
          >
            <Save size={17} />
            {useProofLabel}
          </button>
        </div>
      )}

      {vaultAvailable && (
        <button className="secondaryAction dangerAction" disabled={pending} onClick={removeVault} type="button">
          <Trash2 size={17} />
          {clearVaultLabel}
        </button>
      )}
    </div>
  );
}
