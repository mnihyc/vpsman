import { ConfirmationPrompt } from "./ConfirmationPrompt";

export function PrivilegeLockPrompt({
  onCancel,
  onConfirm,
  open,
}: {
  onCancel: () => void;
  onConfirm: () => void;
  open: boolean;
}) {
  return (
    <ConfirmationPrompt
      confirmLabel="Lock privilege"
      detail="This clears the saved privilege unlock and locks privileged actions. Your signed-in session, current page, drafts, and encrypted local vault stay unchanged."
      onCancel={onCancel}
      onConfirm={onConfirm}
      open={open}
      title="Confirm privilege lock"
      tone="normal"
    />
  );
}
