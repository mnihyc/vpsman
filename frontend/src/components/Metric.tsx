export function Metric({
  label,
  title,
  value,
  tone,
}: {
  label: string;
  title?: string;
  value: string;
  tone: "blue" | "green" | "neutral" | "yellow";
}) {
  return (
    <div className={`metric ${tone}`} title={title ?? `${label}: ${value}.`}>
      <span>{label}</span>
      <strong>{value}</strong>
    </div>
  );
}
