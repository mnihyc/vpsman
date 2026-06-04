import React, { useState } from "react";
import { KeyRound, LockKeyhole } from "lucide-react";
import type { AuthResponse } from "../types";
import { loadAuthVault } from "../vault";

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
  const [mode, setMode] = useState<"login" | "bootstrap">("login");
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [totpCode, setTotpCode] = useState("");
  const [sessionVaultKey, setSessionVaultKey] = useState("");
  const [storedSessionKey, setStoredSessionKey] = useState("");
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(apiError);

  async function submit(event: React.FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setPending(true);
    setError(null);
    try {
      const body: Record<string, string> = { username, password };
      if (mode === "login" && totpCode.trim()) {
        body.totp_code = totpCode.trim();
      }
      const response = await fetch(mode === "login" ? "/api/v1/auth/login" : "/api/v1/auth/bootstrap", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(body),
      });
      if (!response.ok) {
        throw new Error(response.status === 409 ? "Operator already exists" : "Authentication failed");
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
    <section className="authWorkspace">
      <form className="authPanel" onSubmit={submit}>
        <div className="sectionHeader">
          <div>
            <h2>Operator access</h2>
            <span>{error ?? "Bearer session required"}</span>
          </div>
          <div className="segmented">
            <button className={mode === "login" ? "selected" : ""} onClick={() => setMode("login")} type="button">
              Login
            </button>
            <button
              className={mode === "bootstrap" ? "selected" : ""}
              onClick={() => setMode("bootstrap")}
              type="button"
            >
              Bootstrap
            </button>
          </div>
        </div>
        <label>
          <span>Username</span>
          <input autoComplete="username" onChange={(event) => setUsername(event.target.value)} value={username} />
        </label>
        {mode === "login" && (
          <label>
            <span>TOTP code</span>
            <input
              autoComplete="one-time-code"
              inputMode="numeric"
              maxLength={6}
              onChange={(event) => setTotpCode(event.target.value)}
              value={totpCode}
            />
          </label>
        )}
        <label>
          <span>Password</span>
          <input
            autoComplete={mode === "login" ? "current-password" : "new-password"}
            onChange={(event) => setPassword(event.target.value)}
            type="password"
            value={password}
          />
        </label>
        <label>
          <span>Session vault key</span>
          <input
            autoComplete="new-password"
            onChange={(event) => setSessionVaultKey(event.target.value)}
            type="password"
            value={sessionVaultKey}
          />
        </label>
        <button
          aria-label={mode === "login" ? "Submit login" : "Submit bootstrap"}
          className="wideAction"
          disabled={pending || !username || password.length < 12}
          type="submit"
        >
          <KeyRound size={18} />
          <span>{pending ? "Working" : mode === "login" ? "Login" : "Bootstrap"}</span>
        </button>
        {sessionVaultAvailable && (
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
