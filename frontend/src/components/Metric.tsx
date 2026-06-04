export function Metric({ label, value, tone }: { label: string; value: string; tone: "blue" | "green" | "yellow" }) {
  return (
    <div className={`metric ${tone}`}>
      <span>{label}</span>
      <strong>{value}</strong>
    </div>
  );
}
