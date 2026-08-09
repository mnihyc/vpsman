import { AlertTriangle, CheckCircle2, CircleDashed } from "lucide-react";

type ReleaseStatusPanelProps = {
  title: string;
  description: string;
  ready?: readonly string[];
  pending?: readonly string[];
  blocked?: readonly string[];
};

export function ReleaseStatusPanel({
  title,
  description,
  ready = [],
  pending = [],
  blocked = [],
}: ReleaseStatusPanelProps) {
  return (
    <section className="workspace singleColumn">
      <div
        className="releaseStatusPanel"
        aria-label={`${title} release status`}
      >
        <div className="sectionHeader">
          <div>
            <h2>{title}</h2>
            <span>{description}</span>
          </div>
        </div>
        <div className="releaseStatusGrid">
          <ReleaseStatusList
            icon={<CheckCircle2 size={18} />}
            items={ready}
            title="Available now"
            tone="ready"
          />
          <ReleaseStatusList
            icon={<CircleDashed size={18} />}
            items={pending}
            title="Not available yet"
            tone="pending"
          />
          <ReleaseStatusList
            icon={<AlertTriangle size={18} />}
            items={blocked}
            title="Data contract needed"
            tone="blocked"
          />
        </div>
      </div>
    </section>
  );
}

function ReleaseStatusList({
  icon,
  items,
  title,
  tone,
}: {
  icon: JSX.Element;
  items: readonly string[];
  title: string;
  tone: "ready" | "pending" | "blocked";
}) {
  return (
    <section
      className={`releaseStatusList ${tone}`}
      title={`${title}: ${items.length} ${items.length === 1 ? "item" : "items"}.`}
    >
      <div
        className="releaseStatusListTitle"
        title={`${title} release category.`}
      >
        {icon}
        <strong>{title}</strong>
      </div>
      {items.length === 0 ? (
        <p title={`No items are recorded in the ${title} category.`}>
          No items recorded for this category.
        </p>
      ) : (
        <ul>
          {items.map((item) => (
            <li key={item} title={item}>
              {item}
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}
