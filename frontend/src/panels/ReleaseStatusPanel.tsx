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
      <div className="releaseStatusPanel" aria-label={`${title} release status`}>
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
            title="Implementation work"
            tone="pending"
          />
          <ReleaseStatusList
            icon={<AlertTriangle size={18} />}
            items={blocked}
            title="Backend or model contract needed"
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
    <section className={`releaseStatusList ${tone}`}>
      <div className="releaseStatusListTitle">
        {icon}
        <strong>{title}</strong>
      </div>
      {items.length === 0 ? (
        <p>No items recorded for this category.</p>
      ) : (
        <ul>
          {items.map((item) => (
            <li key={item}>{item}</li>
          ))}
        </ul>
      )}
    </section>
  );
}
