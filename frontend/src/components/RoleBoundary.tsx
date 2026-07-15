import { LockKeyhole } from "lucide-react";
import { ConsoleStatusBadge } from "./ConsoleLayout";

export function AdminRoleBoundary({
  currentRole,
  detail,
  title,
}: {
  currentRole: string | null | undefined;
  detail: string;
  title: string;
}) {
  const roleKnown = Boolean(currentRole);

  return (
    <section
      aria-label={`${title} access boundary`}
      className="controlPanel"
    >
      <div className="sectionHeader compact">
        <div>
          <h2>{title}</h2>
          <span>Control-plane authority boundary</span>
        </div>
        <ConsoleStatusBadge tone="neutral">
          {roleKnown ? "Admin only" : "Checking role"}
        </ConsoleStatusBadge>
      </div>
      <div className="emptyState compactEmpty" role="status">
        <LockKeyhole size={20} />
        <strong>{roleKnown ? "Admin role required" : "Checking operator role"}</strong>
        <span>
          {roleKnown
            ? `${detail} Current role: ${currentRole}.`
            : "The current operator profile is still loading. This surface stays closed until its role is known."}
        </span>
      </div>
    </section>
  );
}
