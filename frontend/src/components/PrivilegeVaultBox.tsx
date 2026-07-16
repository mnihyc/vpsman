import { useState } from "react";
import { LockKeyhole, Save, ShieldCheck, Trash2 } from "lucide-react";
import { ActionFeedback } from "./ActionFeedback";
import { ConfirmationPrompt } from "./ConfirmationPrompt";
import { normalizeHex, type PrivilegeMaterial } from "../privilege";
import {
  clearPrivilegeVault,
  hasPrivilegeVault,
  loadPrivilegeVault,
  savePrivilegeVault,
} from "../vault";
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
  showVaultClear?: boolean;
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
  unlockRedirectLabel = "Unlock privilege",
  unlockLabel = "Unlock",
  usePrivilegeLabel = "Unlock privilege",
  showVaultClear = true,
}: PrivilegeVaultBoxProps) {
  const [superPassword, setSuperPassword] = useState("");
  const [superSaltHex, setSuperSaltHex] = useState("");
  const [vaultPassphrase, setVaultPassphrase] = useState("");
  const [unlockPassphrase, setUnlockPassphrase] = useState("");
  const [saveToVault, setSaveToVault] = useState(false);
  const [vaultAvailable, setVaultAvailable] = useState(() =>
    hasPrivilegeVault(),
  );
  const [actionError, setActionError] = useState<string | null>(null);
  const [pending, setPending] = useState(false);
  const [clearVaultPromptOpen, setClearVaultPromptOpen] = useState(false);
  const privilegeStatus = privilegeMaterial
    ? "Unlocked"
    : vaultAvailable
      ? "Locked, saved local vault available"
      : "Locked";
  const unlockScope = privilegeMaterial
    ? "This browser tab"
    : "Current browser only";
  const unlockedUntil = privilegeMaterial
    ? "Until Lock now, refresh, or sign-out"
    : "Not active";
  const localVaultState = vaultAvailable ? "Saved locally" : "Not saved";
  const label = (value: string) => {
    if (!labelPrefix) {
      if (value === "Super password") {
        return "Privilege secret";
      }
      if (value === "Super salt hex") {
        return "Privilege salt";
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

  function vaultClearButton(disabled = false) {
    return (
      <button
        className="secondaryAction dangerAction"
        disabled={pending || disabled}
        onClick={() => setClearVaultPromptOpen(true)}
        type="button"
      >
        <Trash2 size={17} />
        Clear local vault
      </button>
    );
  }

  function clearVaultConfirmation() {
    return (
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
    );
  }

  const stateGrid = (
    <div className="privilegeStateGrid" aria-label="Privilege vault state">
      <span>
        <small>State</small>
        <strong>{privilegeStatus}</strong>
      </span>
      <span>
        <small>Unlock scope</small>
        <strong>{unlockScope}</strong>
      </span>
      <span>
        <small>Unlocked until</small>
        <strong>{unlockedUntil}</strong>
      </span>
      <span>
        <small>Local vault</small>
        <strong>{localVaultState}</strong>
      </span>
    </div>
  );

  if (privilegeMaterial) {
    return (
      <div className="privilegeManager privilegeVaultWorkflow compactPrivilegeManager">
        {stateGrid}
        <div className="privilegeVaultNotice">
          <ShieldCheck size={17} />
          <span>
            <strong>Request-bound privilege assertions</strong>
            <small>
              The server receives signed assertions for privileged actions, not
              the saved secret or vault passphrase.
            </small>
          </span>
        </div>
        <div className="privilegeActionRow">
          <button
            className="secondaryAction dangerAction"
            onClick={lockPrivilege}
            type="button"
          >
            <LockKeyhole size={17} />
            {lockPrivilegeLabel}
          </button>
          {showVaultClear && vaultClearButton(!vaultAvailable)}
        </div>
        {clearVaultConfirmation()}
      </div>
    );
  }

  if (onOpenUnlock) {
    return (
      <div className="privilegeManager">
        <div className="privilegeStatus">
          <ShieldCheck size={18} />
          <div>
            <strong>{privilegeStatus}</strong>
            <span>
              {lastPayloadHash
                ? shortHash(lastPayloadHash)
                : "Access / Privilege Vault required"}
            </span>
          </div>
        </div>
        <button
          className="secondaryAction"
          onClick={onOpenUnlock}
          type="button"
        >
          <LockKeyhole size={17} />
          {unlockRedirectLabel}
        </button>
        <ActionFeedback message={actionError} tone="danger" />
      </div>
    );
  }

  return (
    <div className="privilegeManager privilegeVaultWorkflow">
      {stateGrid}
      <div className="privilegeVaultNotice">
        <ShieldCheck size={18} />
        <span>
          <strong>Local-only privilege material</strong>
          <small>
            Saved material is encrypted in this browser; the API receives only
            request-bound assertions, never this secret or vault passphrase.
          </small>
        </span>
      </div>

      <div className="privilegeForms">
        {vaultAvailable && (
          <form
            className="privilegeVaultSection"
            aria-label="Unlock saved local vault"
            autoComplete="off"
            onSubmit={(event) => {
              event.preventDefault();
              void unlockVault();
            }}
          >
            <div>
              <h3>Unlock saved local vault</h3>
              <p>Use the browser-local vault passphrase to unlock this tab.</p>
            </div>
            <input
              aria-label={label("Vault passphrase")}
              autoComplete="off"
              onChange={(event) => setUnlockPassphrase(event.target.value)}
              placeholder="local vault passphrase"
              type="password"
              value={unlockPassphrase}
            />
            <button
              className="secondaryAction"
              disabled={pending || !unlockPassphrase}
              type="submit"
            >
              <LockKeyhole size={17} />
              {unlockLabel}
            </button>
          </form>
        )}
        <form
          className="privilegeVaultSection"
          aria-label="Unlock with privilege material"
          autoComplete="off"
          onSubmit={(event) => {
            event.preventDefault();
            void activateEnteredPrivilege();
          }}
        >
          <div>
            <h3>Unlock for this browser session</h3>
            <p>
              Enter the privilege material only when a privileged workflow needs
              it. Routine read-only work stays separate.
            </p>
          </div>
          <div className="privilegeFields">
            <label>
              <span>Privilege secret</span>
              <input
                aria-label={label("Super password")}
                autoComplete="off"
                onChange={(event) => setSuperPassword(event.target.value)}
                placeholder="enter privilege secret"
                type="password"
                value={superPassword}
              />
            </label>
            <label title="The 64-character hex salt from operator-privilege.env (VPSMAN_SUPER_SALT_HEX).">
              <span>Privilege salt</span>
              <input
                aria-label={label("Super salt hex")}
                autoComplete="off"
                onChange={(event) => setSuperSaltHex(event.target.value)}
                placeholder="paste 64-character hex salt"
                value={superSaltHex}
              />
            </label>
          </div>
          <label className="checkLine vaultSaveOption">
            <input
              checked={saveToVault}
              onChange={(event) => setSaveToVault(event.target.checked)}
              type="checkbox"
            />
            <span>
              <strong>Keep encrypted in this browser</strong>
              <small>
                Protected by a local passphrase; the server never receives the
                saved material.
              </small>
            </span>
          </label>
          {saveToVault && (
            <input
              aria-label={label("New vault passphrase")}
              autoComplete="off"
              onChange={(event) => setVaultPassphrase(event.target.value)}
              placeholder="new local vault passphrase"
              type="password"
              value={vaultPassphrase}
            />
          )}
          <button
            className="primaryAction"
            disabled={
              pending ||
              !superPassword ||
              !superSaltHex ||
              (saveToVault && !vaultPassphrase)
            }
            type="submit"
          >
            <Save size={17} />
            {usePrivilegeLabel}
          </button>
        </form>
      </div>
      <ActionFeedback message={actionError} tone="danger" />

      {showVaultClear && (
        <div className="privilegeActionRow">
          {vaultClearButton(!vaultAvailable)}
        </div>
      )}
      {clearVaultConfirmation()}
    </div>
  );
}
