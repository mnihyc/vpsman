import { Copy } from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import { renderSVG } from "uqr";
import type { TotpSetupResponse } from "../types";

export function TotpEnrollmentQr({ setup }: { setup: TotpSetupResponse }) {
  const [copyStatus, setCopyStatus] = useState<string | null>(null);
  useEffect(() => {
    setCopyStatus(null);
  }, [setup.secret_base32]);

  const qr = useMemo(() => {
    try {
      const svg = renderSVG(setup.otpauth_uri, {
        blackColor: "#17233f",
        border: 2,
        ecc: "M",
        pixelSize: 5,
        whiteColor: "#ffffff",
      });
      return {
        dataUrl: `data:image/svg+xml,${encodeURIComponent(svg)}`,
        error: null,
      };
    } catch {
      return {
        dataUrl: null,
        error:
          "QR generation failed in this browser. Enter the setup key manually.",
      };
    }
  }, [setup.otpauth_uri]);

  async function copySetupKey() {
    try {
      if (!navigator.clipboard?.writeText) {
        throw new Error("Clipboard API unavailable");
      }
      await navigator.clipboard.writeText(setup.secret_base32);
      setCopyStatus("Setup key copied");
    } catch {
      setCopyStatus("Copy failed; select the setup key manually");
    }
  }

  return (
    <section aria-label="Authenticator QR code" className="totpEnrollmentPanel">
      <div className="totpQrFrame">
        {qr.dataUrl ? (
          <img
            alt="QR code for this vpsman authenticator account"
            height={220}
            src={qr.dataUrl}
            width={220}
          />
        ) : (
          <span>{qr.error}</span>
        )}
      </div>
      <div className="totpEnrollmentInstructions">
        <strong>Scan with your authenticator</strong>
        <span>
          Add an account by QR code, then enter its current six-digit code
          below.
        </span>
        <div className="totpManualKey">
          <span>
            <small>Manual setup key</small>
            <code data-tooltip-sensitive="true" data-value-tooltip-skip="true">
              {setup.secret_base32}
            </code>
          </span>
          <button
            className="secondaryAction compactAction"
            onClick={() => void copySetupKey()}
            type="button"
          >
            <Copy size={15} />
            Copy key
          </button>
        </div>
        <small aria-live="polite" className="totpCopyStatus">
          {copyStatus ??
            `${setup.algorithm} · ${setup.digits} digits · ${setup.period_secs}-second period`}
        </small>
      </div>
    </section>
  );
}
