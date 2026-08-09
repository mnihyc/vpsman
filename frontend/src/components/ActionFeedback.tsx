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

const toneTitle: Record<ActionFeedbackTone, string> = {
  danger:
    "Action error feedback. Exact server detail is displayed here and excluded from tooltips.",
  info:
    "Action information. Exact detail is displayed here and excluded from tooltips.",
  progress:
    "Action progress feedback. Exact detail is displayed here and excluded from tooltips.",
  success:
    "Action success feedback. Exact detail is displayed here and excluded from tooltips.",
  warning:
    "Action warning feedback. Exact server detail is displayed here and excluded from tooltips.",
};

export const ActionFeedback = forwardRef<HTMLDivElement, {
  className?: string;
  id?: string;
  message: string | null | undefined;
  tone?: ActionFeedbackTone;
}>(function ActionFeedback(
  {
    className,
    id,
    message,
    tone = "info",
  },
  ref,
) {
  if (!message) {
    return null;
  }
  const role = tone === "danger" ? "alert" : "status";
  const ariaLive = tone === "danger" ? "assertive" : "polite";
  return (
    <div
      aria-live={ariaLive}
      className={`authNotice actionFeedback ${toneClass[tone]}${className ? ` ${className}` : ""}`}
      data-tooltip-sensitive="true"
      data-value-tooltip-skip="true"
      id={id}
      ref={ref}
      role={role}
      title={toneTitle[tone]}
    >
      <span>{message}</span>
    </div>
  );
});
import { forwardRef } from "react";
