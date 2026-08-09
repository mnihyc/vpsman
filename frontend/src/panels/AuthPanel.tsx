import React, { useEffect, useRef, useState } from "react";
import { KeyRound } from "lucide-react";
import { apiErrorFromResponse, apiFetch, apiJsonFromResponse } from "../api";
import type { AuthResponse } from "../types";

type AuthMode = "checking" | "login" | "bootstrap";

type BootstrapStatusResponse = {
  bootstrap_required: boolean;
};

export function AuthPanel({
  apiError,
  onAuth,
  sessionNotice,
}: {
  apiError: string | null;
  onAuth: (auth: AuthResponse) => Promise<void>;
  sessionNotice: string | null;
}) {
  const [mode, setMode] = useState<AuthMode>("checking");
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [totpCode, setTotpCode] = useState("");
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(apiError);
  const pendingRef = useRef(false);
  const passwordInputRef = useRef<HTMLInputElement | null>(null);
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
        const response = await apiFetch("/api/v1/auth/bootstrap-status", {
          headers: { Accept: "application/json" },
        });
        if (!response.ok) {
          throw await apiErrorFromResponse(response);
        }
        const status = await apiJsonFromResponse<BootstrapStatusResponse>(
          response,
          "GET /api/v1/auth/bootstrap-status",
        );
        if (!canceled) {
          setMode(status.bootstrap_required ? "bootstrap" : "login");
          setError(null);
        }
      } catch (bootstrapError) {
        if (!canceled) {
          setMode("login");
          setError(
            `Could not verify first-run state. ${
              bootstrapError instanceof Error
                ? bootstrapError.message
                : "The browser returned no failure detail. Check API availability and refresh."
            } Sign in only if this control plane is already initialized.`,
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
    if (mode === "checking" || pendingRef.current) {
      return;
    }
    pendingRef.current = true;
    setPending(true);
    setError(null);
    try {
      const body: Record<string, string> = { username, password };
      if (mode === "login" && totpCode.trim()) {
        body.totp_code = totpCode.trim();
      }
      const response = await apiFetch(
        mode === "login" ? "/api/v1/auth/login" : "/api/v1/auth/bootstrap",
        {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify(body),
        },
      );
      if (!response.ok) {
        const message = authErrorMessage(response.status, mode);
        throw message
          ? new Error(message)
          : await apiErrorFromResponse(response);
      }
      await onAuth(
        await apiJsonFromResponse<AuthResponse>(
          response,
          `${mode === "login" ? "POST /api/v1/auth/login" : "POST /api/v1/auth/bootstrap"}`,
        ),
      );
      setPassword("");
      setTotpCode("");
    } catch (authError) {
      setError(
        authError instanceof Error
          ? authError.message
          : "Authentication did not return a usable result. Refresh first-run state and inspect the browser console or API logs before retrying.",
      );
      window.requestAnimationFrame(() => {
        passwordInputRef.current?.focus();
        passwordInputRef.current?.select();
      });
    } finally {
      pendingRef.current = false;
      setPending(false);
    }
  }

  return (
    <section className="authWorkspace" aria-labelledby="operator-access-title">
      <form
        aria-describedby={`auth-mode-summary operator-access-status auth-submit-requirements${
          sessionNotice ? " auth-session-notice" : ""
        }`}
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
              <span
                className="authStatePill"
                title={`Authentication state: ${authModeSummaryTitle(mode)}.`}
              >
                {authModeSummaryTitle(mode)}
              </span>
              <span title={authModeSummaryDetail(mode)}>
                {authModeSummaryDetail(mode)}
              </span>
            </div>
          </div>
        </div>
        {sessionNotice ? (
          <div
            aria-live="polite"
            className="authNotice"
            id="auth-session-notice"
            role="alert"
          >
            {sessionNotice}
          </div>
        ) : null}
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
            data-tooltip-disabled-reason="Username entry is unavailable while the console checks whether first-run setup is required."
            disabled={isChecking}
            id="operator-username"
            name="username"
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
              data-tooltip-sensitive="true"
              id="operator-totp-code"
              inputMode="numeric"
              maxLength={6}
              name="totp_code"
              placeholder="Optional if MFA is disabled"
              onChange={(event) => setTotpCode(event.target.value)}
              value={totpCode}
            />
          </label>
        )}
        <label>
          <span>Password</span>
          <input
            autoComplete={
              mode === "login" ? "current-password" : "new-password"
            }
            data-tooltip-disabled-reason="Password entry is unavailable while the console checks whether first-run setup is required."
            data-tooltip-sensitive="true"
            disabled={isChecking}
            id="operator-password"
            name="password"
            onChange={(event) => setPassword(event.target.value)}
            ref={passwordInputRef}
            type="password"
            value={password}
          />
          <small className="fieldHelp">
            {passwordRequirementMessage(password, isBootstrap)}
          </small>
        </label>
        <button
          aria-describedby="auth-submit-requirements"
          className="wideAction"
          data-tooltip-disabled-reason={authSubmitDisabledReason(
            mode,
            pending,
            username,
            password,
          )}
          disabled={submitDisabled}
          title={
            submitDisabled
              ? authSubmitDisabledReason(mode, pending, username, password)
              : undefined
          }
          type="submit"
        >
          <KeyRound size={18} />
          <span>{pending ? pendingLabel : submitLabel}</span>
        </button>
        <span className="visuallyHidden" id="auth-submit-requirements">
          {mode === "bootstrap"
            ? "First operator creation needs a username and a password of at least 12 characters."
            : "Sign in needs a username and a password of at least 12 characters."}
        </span>
      </form>
    </section>
  );
}

function authSubmitDisabledReason(
  mode: AuthMode,
  pending: boolean,
  username: string,
  password: string,
): string {
  if (mode === "checking") {
    return "Authentication is unavailable while the console checks whether first-run setup is required.";
  }
  if (pending) {
    return mode === "bootstrap"
      ? "First operator creation is already in progress."
      : "Sign-in is already in progress.";
  }
  if (!username) {
    return "Enter an operator username before continuing.";
  }
  if (password.length < 12) {
    return "Enter a password of at least 12 characters before continuing.";
  }
  return "Authentication is ready.";
}

function authHeaderSubtitle(mode: AuthMode): string {
  if (mode === "bootstrap") {
    return "Set up initial admin access.";
  }
  if (mode === "login") {
    return "Sign in with an existing operator account.";
  }
  return "Selecting the correct access screen.";
}

function passwordRequirementMessage(
  password: string,
  isBootstrap: boolean,
): string {
  if (password.length === 0) {
    return isBootstrap
      ? "Use at least 12 characters for the first admin password."
      : "Enter the operator password (at least 12 characters).";
  }
  if (password.length < 12) {
    const remaining = 12 - password.length;
    return `${remaining} more character${remaining === 1 ? "" : "s"} required.`;
  }
  return "Password length requirement met.";
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

function authErrorMessage(status: number, mode: AuthMode): string | null {
  if (mode === "bootstrap" && status === 409) {
    return "First operator already exists. Refresh and sign in.";
  }
  if (status === 401) {
    return "Username, password, or TOTP code is incorrect.";
  }
  if (status === 429) {
    return "Too many authentication attempts. Wait before trying again.";
  }
  return null;
}
