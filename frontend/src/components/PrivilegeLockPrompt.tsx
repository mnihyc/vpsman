import type { ReactNode } from "react";
import { ConfirmationPrompt } from "./ConfirmationPrompt";

export function PrivilegeLockPrompt({
  error,
  onCancel,
  onConfirm,
  open,
  pending = false,
}: {
  error?: ReactNode;
  onCancel: () => void;
  onConfirm: () => void;
  open: boolean;
  pending?: boolean;
}) {
  return (
    <ConfirmationPrompt
      confirmLabel="Lock privilege"
      detail="This clears the saved privilege unlock and locks privileged actions. Your signed-in session, current page, drafts, and encrypted local vault stay unchanged."
      error={error}
      onCancel={onCancel}
      onConfirm={onConfirm}
      open={open}
      pending={pending}
      title="Confirm privilege lock"
      tone="normal"
    />
  );
}
