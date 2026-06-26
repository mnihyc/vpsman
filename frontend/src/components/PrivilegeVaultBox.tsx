import { useState } from "react";
import { LockKeyhole, Save, ShieldCheck, Trash2 } from "lucide-react";
import { ConfirmationPrompt } from "./ConfirmationPrompt";
import { normalizeHex, type PrivilegeMaterial } from "../privilege";
import { clearPrivilegeVault, hasPrivilegeVault, loadPrivilegeVault, savePrivilegeVault } from "../vault";
import { runPanelAction, shortHash } from "../utils";

type PrivilegeVaultBoxProps = {
  labelPrefix?: string;
  lastPayloadHash: string | null;
  onPrivilegeMaterialChange: (material: PrivilegeMaterial | null) => void;
  onOpenUnlock?: () => void;
  onVaultAvailabilityChange?: (available: boolean) => void;
  privilegeMaterial: PrivilegeMaterial | null;
  clearVaultLabel?: string;
  lockPrivilegeLabel?: string;
  unlockRedirectLabel?: string;
  unlockLabel?: string;
  usePrivilegeLabel?: string;
};

export function PrivilegeVaultBox({
  clearVaultLabel = "Clear vault",
  labelPrefix = "",
  lastPayloadHash,
  lockPrivilegeLabel = "Lock privilege",
  onOpenUnlock,
  onPrivilegeMaterialChange,
  onVaultAvailabilityChange,
  privilegeMaterial,
  unlockRedirectLabel = "Open Privilege Vault",
  unlockLabel = "Unlock",
  usePrivilegeLabel = "Unlock privilege",
}: PrivilegeVaultBoxProps) {
  const [superPassword, setSuperPassword] = useState("");
  const [superSaltHex, setSuperSaltHex] = useState("");
  const [vaultPassphrase, setVaultPassphrase] = useState("");
  const [unlockPassphrase, setUnlockPassphrase] = useState("");
  const [saveToVault, setSaveToVault] = useState(false);
  const [vaultAvailable, setVaultAvailable] = useState(() => hasPrivilegeVault());
  const [actionError, setActionError] = useState<string | null>(null);
  const [pending, setPending] = useState(false);
  const [clearVaultPromptOpen, setClearVaultPromptOpen] = useState(false);
  const privilegeStatus = vaultAvailable ? "Encrypted vault locked" : "Locked";
  const label = (value: string) => {
    if (!labelPrefix) {
      if (value === "Super password") {
        return "Privilege secret";
      }
      if (value === "Super salt hex") {
        return "Verifier salt hex";
      }
      return value;
    }
    if (value === "Super password") {
      return `${labelPrefix} privilege secret`;
    }
    if (value === "Super salt hex") {
      return `${labelPrefix} privilege salt`;
    }
    return `${labelPrefix} privilege ${value.toLowerCase()}`;
  };

  async function unlockVault() {
    await runPanelAction(setPending, setActionError, async () => {
      onPrivilegeMaterialChange(await loadPrivilegeVault(unlockPassphrase));
      setUnlockPassphrase("");
    });
  }

  async function activateEnteredPrivilege() {
    await runPanelAction(setPending, setActionError, async () => {
      const material = {
        superPassword,
        superSaltHex: normalizeHex(superSaltHex),
      };
      if (saveToVault) {
        await savePrivilegeVault(material, vaultPassphrase);
        setVaultAvailable(true);
        onVaultAvailabilityChange?.(true);
        setVaultPassphrase("");
      }
      onPrivilegeMaterialChange(material);
      setSuperPassword("");
      setSuperSaltHex("");
    });
  }

  function lockPrivilege() {
    onPrivilegeMaterialChange(null);
    setActionError(null);
  }

  function removeVault() {
    setClearVaultPromptOpen(false);
    clearPrivilegeVault();
    setVaultAvailable(false);
    onVaultAvailabilityChange?.(false);
    onPrivilegeMaterialChange(null);
    setActionError(null);
  }

  if (privilegeMaterial) {
    return (
      <div className="privilegeManager compactPrivilegeManager">
        <button className="secondaryAction" onClick={lockPrivilege} type="button">
          <LockKeyhole size={17} />
          {lockPrivilegeLabel}
        </button>
      </div>
    );
  }

  if (onOpenUnlock) {
    return (
      <div className="privilegeManager">
        <div className="privilegeStatus">
          <ShieldCheck size={18} />
          <div>
            <strong>{actionError ?? privilegeStatus}</strong>
            <span>{lastPayloadHash ? shortHash(lastPayloadHash) : "Access / Privilege Vault required"}</span>
          </div>
        </div>
        <button className="secondaryAction" onClick={onOpenUnlock} type="button">
          <LockKeyhole size={17} />
          {unlockRedirectLabel}
        </button>
      </div>
    );
  }

  return (
    <div className="privilegeManager">
      <div className="privilegeStatus">
        <ShieldCheck size={18} />
        <div>
          <strong>{actionError ?? privilegeStatus}</strong>
          <span>{lastPayloadHash ? shortHash(lastPayloadHash) : "Local privilege unlock"}</span>
        </div>
      </div>

      <div className="privilegeForms">
        {vaultAvailable && (
          <div className="inlinePrivilege">
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
        <div className="privilegeFields">
          <input
            aria-label={label("Super password")}
            onChange={(event) => setSuperPassword(event.target.value)}
            placeholder="privilege secret"
            type="password"
            value={superPassword}
          />
          <input
            aria-label={label("Super salt hex")}
            onChange={(event) => setSuperSaltHex(event.target.value)}
            placeholder="verifier salt hex"
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
          onClick={activateEnteredPrivilege}
          type="button"
        >
          <Save size={17} />
          {usePrivilegeLabel}
        </button>
      </div>

      {vaultAvailable && (
        <>
          <button className="secondaryAction dangerAction" disabled={pending} onClick={() => setClearVaultPromptOpen(true)} type="button">
            <Trash2 size={17} />
            Review vault clear
          </button>
          <ConfirmationPrompt
            confirmLabel={clearVaultLabel}
            detail="This removes the encrypted local privilege vault from this browser and locks locally cached privilege material."
            onCancel={() => setClearVaultPromptOpen(false)}
            onConfirm={removeVault}
            open={clearVaultPromptOpen}
            pending={pending}
            title="Confirm privilege vault clear"
            tone="danger"
          />
        </>
      )}
    </div>
  );
}
