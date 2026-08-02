import { useId, useState } from "react";
import { LockKeyhole, Save, ShieldCheck, Trash2 } from "lucide-react";
import { ActionFeedback } from "./ActionFeedback";
import { ConfirmationPrompt } from "./ConfirmationPrompt";
import { PrivilegeLockPrompt } from "./PrivilegeLockPrompt";
import { normalizeHex, type PrivilegeMaterial } from "../privilege";
import {
  clearPrivilegeVault,
  hasPrivilegeVault,
  loadPrivilegeVault,
  MIN_VAULT_PASSPHRASE_LENGTH,
  savePrivilegeVault,
} from "../vault";
import { runPanelAction, shortHash } from "../utils";

type PrivilegeVaultBoxProps = {
  labelPrefix?: string;
  lastPayloadHash: string | null;
  onPrivilegeMaterialChange: (
    material: PrivilegeMaterial | null,
  ) => Promise<void>;
  onOpenUnlock?: () => void;
  onUnlocked?: () => void;
  onVaultAvailabilityChange?: (available: boolean) => void;
  privilegeMaterial: PrivilegeMaterial | null;
  clearVaultLabel?: string;
  lockPrivilegeLabel?: string;
  unlockRedirectLabel?: string;
  unlockLabel?: string;
  usePrivilegeLabel?: string;
  showVaultClear?: boolean;
  showHandoffState?: boolean;
};

export function PrivilegeVaultBox({
  clearVaultLabel = "Clear vault",
  labelPrefix = "",
  lastPayloadHash,
  lockPrivilegeLabel = "Lock privilege",
  onOpenUnlock,
  onPrivilegeMaterialChange,
  onUnlocked,
  onVaultAvailabilityChange,
  privilegeMaterial,
  unlockRedirectLabel = "Unlock privilege",
  unlockLabel = "Unlock",
  usePrivilegeLabel = "Unlock privilege",
  showVaultClear = true,
  showHandoffState = false,
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
  const [lockPromptOpen, setLockPromptOpen] = useState(false);
  const vaultPassphraseHintId = useId();
  const privilegeStatus = privilegeMaterial
    ? "Verified and unlocked"
    : !showHandoffState && vaultAvailable
      ? "Locked, saved local vault available"
      : "Locked";
  const unlockScope = privilegeMaterial
    ? "This browser, including restarts"
    : showHandoffState
      ? "This browser after verification"
      : "Current browser only";
  const unlockedUntil = privilegeMaterial
    ? "Until Lock, sign-out, or operator change"
    : "Not active";
  const localVaultState = showHandoffState
    ? privilegeMaterial
      ? "Derived capability saved locally"
      : "No saved unlock"
    : vaultAvailable
      ? "Saved locally"
      : "Not saved";
  const label = (value: string) => {
    if (!labelPrefix) {
      if (value === "Super password") {
        return "Super password";
      }
      if (value === "Super salt hex") {
        return "Privilege salt";
      }
      return value;
    }
    if (value === "Super password") {
      return `${labelPrefix} super password`;
    }
    if (value === "Super salt hex") {
      return `${labelPrefix} privilege salt`;
    }
    return `${labelPrefix} privilege ${value.toLowerCase()}`;
  };

  async function unlockVault() {
    await runPanelAction(setPending, setActionError, async () => {
      const material = await loadPrivilegeVault(unlockPassphrase);
      await onPrivilegeMaterialChange(material);
      setUnlockPassphrase("");
      onUnlocked?.();
    });
  }

  async function activateEnteredPrivilege() {
    await runPanelAction(setPending, setActionError, async () => {
      const material = {
        superPassword,
        superSaltHex: normalizeHex(superSaltHex),
      };
      await onPrivilegeMaterialChange(material);
      try {
        if (saveToVault) {
          await savePrivilegeVault(material, vaultPassphrase);
          setVaultAvailable(true);
          onVaultAvailabilityChange?.(true);
          setVaultPassphrase("");
        }
      } catch (error) {
        await onPrivilegeMaterialChange(null);
        throw error;
      }
      setSuperPassword("");
      setSuperSaltHex("");
      onUnlocked?.();
    });
  }

  async function lockPrivilege() {
    setLockPromptOpen(false);
    await runPanelAction(setPending, setActionError, async () => {
      await onPrivilegeMaterialChange(null);
    });
  }

  async function removeVault() {
    setClearVaultPromptOpen(false);
    await runPanelAction(setPending, setActionError, async () => {
      clearPrivilegeVault();
      setVaultAvailable(false);
      onVaultAvailabilityChange?.(false);
      await onPrivilegeMaterialChange(null);
    });
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
        onConfirm={() => void removeVault()}
        open={clearVaultPromptOpen}
        pending={pending}
        title="Confirm privilege vault clear"
        tone="danger"
      />
    );
  }

  const lockConfirmation = (
    <PrivilegeLockPrompt
      onCancel={() => setLockPromptOpen(false)}
      onConfirm={() => void lockPrivilege()}
      open={lockPromptOpen}
    />
  );

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
        <small>{showHandoffState ? "Saved unlock" : "Local vault"}</small>
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
            <strong>
              {showHandoffState
                ? "Persistent browser unlock"
                : "Request-bound privilege assertions"}
            </strong>
            <small>
              {showHandoffState
                ? "The password entry is cleared after verification; only the derived signing capability is saved locally, and the API receives request-bound assertions."
                : "The server receives signed assertions for privileged actions, not the saved secret or vault passphrase."}
            </small>
          </span>
        </div>
        <div className="privilegeActionRow">
          <button
            className="secondaryAction"
            onClick={() => setLockPromptOpen(true)}
            type="button"
          >
            <LockKeyhole size={17} />
            {lockPrivilegeLabel}
          </button>
          {showVaultClear && vaultClearButton(!vaultAvailable)}
        </div>
        {clearVaultConfirmation()}
        {lockConfirmation}
      </div>
    );
  }

  if (onOpenUnlock) {
    return (
      <div
        className={`privilegeManager${showHandoffState ? " privilegeVaultWorkflow" : ""}`}
      >
        {showHandoffState ? (
          <>
            {stateGrid}
            <div className="privilegeVaultNotice">
              <ShieldCheck size={17} />
              <span>
                <strong>Persistent browser unlock</strong>
                <small>
                  Unlock is verified before a derived signing capability is
                  saved for this browser. Lock, sign-out, or an operator change
                  clears it.
                </small>
              </span>
            </div>
          </>
        ) : (
          <div className="privilegeStatus">
            <ShieldCheck size={18} />
            <div>
              <strong>{privilegeStatus}</strong>
              <span title={lastPayloadHash ?? undefined}>
                {lastPayloadHash
                  ? shortHash(lastPayloadHash)
                  : "Access / Privilege Vault required"}
              </span>
            </div>
          </div>
        )}
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
              <p>Use the browser-local vault passphrase to unlock privilege.</p>
            </div>
            <input
              aria-label={label("Vault passphrase")}
              autoComplete="off"
              name="vault_unlock_passphrase"
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
            <h3>Unlock in this browser</h3>
            <p>
              Enter the privilege material only when a privileged workflow needs
              it. Routine read-only work stays separate.
            </p>
          </div>
          <div className="privilegeFields">
            <label>
              <span>Super password</span>
              <input
                aria-label={label("Super password")}
                autoComplete="off"
                name="privilege_secret"
                onChange={(event) => setSuperPassword(event.target.value)}
                placeholder="enter super password"
                type="password"
                value={superPassword}
              />
            </label>
            <label title="The 64-character hex salt from operator-privilege.env (VPSMAN_SUPER_SALT_HEX).">
              <span>Privilege salt</span>
              <input
                aria-label={label("Super salt hex")}
                autoComplete="off"
                name="privilege_salt_hex"
                onChange={(event) => setSuperSaltHex(event.target.value)}
                placeholder="paste salt printed by first-start"
                value={superSaltHex}
              />
            </label>
          </div>
          <label className="checkLine vaultSaveOption">
            <input
              checked={saveToVault}
              name="save_privilege_to_vault"
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
            <>
              <input
                aria-describedby={vaultPassphraseHintId}
                aria-label={label("New vault passphrase")}
                autoComplete="off"
                minLength={MIN_VAULT_PASSPHRASE_LENGTH}
                name="new_vault_passphrase"
                onChange={(event) => setVaultPassphrase(event.target.value)}
                placeholder="new local vault passphrase"
                type="password"
                value={vaultPassphrase}
              />
              <small id={vaultPassphraseHintId}>
                Use at least {MIN_VAULT_PASSPHRASE_LENGTH} characters. New vaults
                use 600,000 PBKDF2-SHA256 iterations; existing vaults remain
                unlockable with their recorded parameters.
              </small>
            </>
          )}
          <button
            className="primaryAction"
            disabled={
              pending ||
              !superPassword ||
              !superSaltHex ||
              (saveToVault &&
                vaultPassphrase.length < MIN_VAULT_PASSPHRASE_LENGTH)
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
      {lockConfirmation}
    </div>
  );
}
