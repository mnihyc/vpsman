export type ActionFeedbackTone =
  | "danger"
  | "info"
  | "progress"
  | "success"
  | "warning";

const toneClass: Record<ActionFeedbackTone, string> = {
  danger: "actionFeedbackDanger",
  info: "actionFeedbackInfo",
  progress: "actionFeedbackProgress",
  success: "actionFeedbackSuccess",
  warning: "actionFeedbackWarning",
};

export function ActionFeedback({
  className,
  id,
  message,
  tone = "info",
}: {
  className?: string;
  id?: string;
  message: string | null | undefined;
  tone?: ActionFeedbackTone;
}) {
  if (!message) {
    return null;
  }
  const role = tone === "danger" ? "alert" : "status";
  const ariaLive = tone === "danger" ? "assertive" : "polite";
  return (
    <div
      aria-live={ariaLive}
      className={`authNotice actionFeedback ${toneClass[tone]}${className ? ` ${className}` : ""}`}
      id={id}
      role={role}
    >
      <span>{message}</span>
    </div>
  );
}
