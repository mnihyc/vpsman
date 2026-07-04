import React, { useEffect, useRef, useState } from "react";
import { KeyRound, LockKeyhole } from "lucide-react";
import type { AuthResponse } from "../types";
import { loadAuthVault } from "../vault";

type AuthMode = "checking" | "login" | "bootstrap";

type BootstrapStatusResponse = {
  bootstrap_required: boolean;
};

export function AuthPanel({
  apiError,
  onAuth,
  onSessionUnlock,
  sessionVaultAvailable,
}: {
  apiError: string | null;
  onAuth: (auth: AuthResponse, sessionVaultKey?: string) => Promise<void>;
  onSessionUnlock: (auth: AuthResponse) => void;
  sessionVaultAvailable: boolean;
}) {
  const [mode, setMode] = useState<AuthMode>("checking");
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [totpCode, setTotpCode] = useState("");
  const [sessionVaultKey, setSessionVaultKey] = useState("");
  const [storedSessionKey, setStoredSessionKey] = useState("");
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(apiError);
  const usernameInputRef = useRef<HTMLInputElement | null>(null);
  const isBootstrap = mode === "bootstrap";
  const isChecking = mode === "checking";
  const submitLabel = isBootstrap ? "Create first operator" : "Sign in";
  const pendingLabel = isBootstrap ? "Creating" : "Signing in";
  const submitDisabled =
    isChecking || pending || !username || password.length < 12;

  useEffect(() => {
    let canceled = false;
    async function loadBootstrapStatus() {
      try {
        const response = await fetch("/api/v1/auth/bootstrap-status", {
          headers: { Accept: "application/json" },
        });
        if (!response.ok) {
          throw new Error("bootstrap_status_unavailable");
        }
        const status = (await response.json()) as BootstrapStatusResponse;
        if (!canceled) {
          setMode(status.bootstrap_required ? "bootstrap" : "login");
          setError(null);
        }
      } catch (_) {
        if (!canceled) {
          setMode("login");
          setError(
            "Could not verify first-run state. Sign in if this control plane is already initialized.",
          );
        }
      }
    }

    void loadBootstrapStatus();
    return () => {
      canceled = true;
    };
  }, []);

  useEffect(() => {
    if (mode !== "checking") {
      usernameInputRef.current?.focus();
    }
  }, [mode]);

  async function submit(event: React.FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (mode === "checking") {
      return;
    }
    setPending(true);
    setError(null);
    try {
      const body: Record<string, string> = { username, password };
      if (mode === "login" && totpCode.trim()) {
        body.totp_code = totpCode.trim();
      }
      const response = await fetch(
        mode === "login" ? "/api/v1/auth/login" : "/api/v1/auth/bootstrap",
        {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify(body),
        },
      );
      if (!response.ok) {
        throw new Error(authErrorMessage(response.status, mode));
      }
      await onAuth((await response.json()) as AuthResponse, sessionVaultKey.trim() || undefined);
      setPassword("");
      setTotpCode("");
      setSessionVaultKey("");
    } catch (authError) {
      setError(authError instanceof Error ? authError.message : "Authentication failed");
    } finally {
      setPending(false);
    }
  }

  async function unlockStoredSession() {
    setPending(true);
    setError(null);
    try {
      onSessionUnlock(await loadAuthVault(storedSessionKey));
      setStoredSessionKey("");
    } catch (authError) {
      setError(authError instanceof Error ? authError.message : "Session unlock failed");
    } finally {
      setPending(false);
    }
  }

  return (
    <section className="authWorkspace" aria-labelledby="operator-access-title">
      <form
        aria-describedby="auth-mode-summary operator-access-status auth-submit-requirements"
        aria-label="Operator authentication"
        className="authPanel"
        onSubmit={submit}
      >
        <div className="sectionHeader authHeader">
          <div>
            <h1 id="operator-access-title">
              {isChecking ? "Checking operator access" : submitLabel}
            </h1>
            <span className="authHeaderSubtitle">
              {authHeaderSubtitle(mode)}
            </span>
            <div className="authModeSummary" id="auth-mode-summary">
              <span className="authStatePill">
                {authModeSummaryTitle(mode)}
              </span>
              <span>{authModeSummaryDetail(mode)}</span>
            </div>
          </div>
        </div>
        {error ? (
          <div
            aria-live="polite"
            className="authNotice"
            id="operator-access-status"
            role="alert"
          >
            {error}
          </div>
        ) : (
          <span className="visuallyHidden" id="operator-access-status">
            {authScreenStatus(mode)}
          </span>
        )}
        <label>
          <span>Username</span>
          <input
            autoComplete="username"
            autoFocus
            disabled={isChecking}
            onChange={(event) => setUsername(event.target.value)}
            ref={usernameInputRef}
            value={username}
          />
        </label>
        {mode === "login" && (
          <label>
            <span>TOTP code</span>
            <input
              autoComplete="one-time-code"
              inputMode="numeric"
              maxLength={6}
              placeholder="Optional if MFA is disabled"
              onChange={(event) => setTotpCode(event.target.value)}
              value={totpCode}
            />
          </label>
        )}
        <label>
          <span>Password</span>
          <input
            autoComplete={mode === "login" ? "current-password" : "new-password"}
            disabled={isChecking}
            onChange={(event) => setPassword(event.target.value)}
            type="password"
            value={password}
          />
        </label>
        <label>
          <span>Session vault key</span>
          <input
            autoComplete="new-password"
            disabled={isChecking}
            onChange={(event) => setSessionVaultKey(event.target.value)}
            placeholder="Optional local key"
            type="password"
            value={sessionVaultKey}
          />
          <small>
            Optional. Encrypts this browser session locally so you can unlock
            it later without storing bearer tokens in plain local storage.
          </small>
        </label>
        <button
          aria-describedby="auth-submit-requirements"
          className="wideAction"
          disabled={submitDisabled}
          title={
            submitDisabled
              ? "Enter username and a password of at least 12 characters."
              : undefined
          }
          type="submit"
        >
          <KeyRound size={18} />
          <span>{pending ? pendingLabel : submitLabel}</span>
        </button>
        <span className="visuallyHidden" id="auth-submit-requirements">
          Sign in and first operator creation need a username and a password of at least 12 characters.
        </span>
        {sessionVaultAvailable && !isChecking && (
          <div className="authVaultUnlock">
            <label>
              <span>Stored session key</span>
              <input
                autoComplete="current-password"
                onChange={(event) => setStoredSessionKey(event.target.value)}
                type="password"
                value={storedSessionKey}
              />
            </label>
            <button
              className="wideAction secondaryWideAction"
              disabled={pending || !storedSessionKey}
              title={
                pending || !storedSessionKey
                  ? "Enter the stored session key to unlock the saved session."
                  : undefined
              }
              onClick={() => void unlockStoredSession()}
              type="button"
            >
              <LockKeyhole size={18} />
              <span>Unlock session</span>
            </button>
          </div>
        )}
      </form>
    </section>
  );
}

function authHeaderSubtitle(mode: AuthMode): string {
  if (mode === "bootstrap") {
    return "Set up initial admin access.";
  }
  if (mode === "login") {
    return "Enter an existing operator session.";
  }
  return "Selecting the correct access screen.";
}

function authScreenStatus(mode: AuthMode): string {
  if (mode === "bootstrap") {
    return "First-run operator creation is required.";
  }
  if (mode === "login") {
    return "Operator sign in is required.";
  }
  return "Checking operator access state.";
}

function authModeSummaryTitle(mode: AuthMode): string {
  if (mode === "bootstrap") {
    return "First run";
  }
  if (mode === "login") {
    return "Initialized";
  }
  return "Checking";
}

function authModeSummaryDetail(mode: AuthMode): string {
  if (mode === "bootstrap") {
    return "This control plane has no operators yet. Create the initial admin operator once, then continue directly into the console.";
  }
  if (mode === "login") {
    return "Use a registered operator account. Enter the TOTP code only when MFA is enabled.";
  }
  return "The console is asking the API which authentication screen is appropriate.";
}

function authErrorMessage(status: number, mode: AuthMode): string {
  if (mode === "bootstrap" && status === 409) {
    return "First operator already exists. Refresh and sign in.";
  }
  if (status === 401) {
    return "Username, password, or TOTP code is incorrect.";
  }
  if (status === 429) {
    return "Too many authentication attempts. Wait before trying again.";
  }
  return mode === "bootstrap"
    ? "First operator creation failed."
    : "Sign in failed.";
}
