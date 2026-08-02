import { useEffect, useRef } from "react";

export type MonitoringWindow =
  | "15m"
  | "1h"
  | "8h"
  | "1d"
  | "7d"
  | "30d"
  | "90d"
  | "180d"
  | "1y"
  | "all"
  | "custom";

const MONITORING_RANGE_OPTIONS: ReadonlyArray<{
  accessibleLabel: string;
  label: string;
  title: string;
  value: MonitoringWindow;
}> = [
  {
    accessibleLabel: "Realtime, last 15 minutes",
    label: "15m",
    title: "Realtime · last 15 minutes.",
    value: "15m",
  },
  {
    accessibleLabel: "Last hour",
    label: "1h",
    title: "Last hour",
    value: "1h",
  },
  {
    accessibleLabel: "Last 8 hours",
    label: "8h",
    title: "Last 8 hours",
    value: "8h",
  },
  { accessibleLabel: "Last day", label: "1d", title: "Last day", value: "1d" },
  {
    accessibleLabel: "Last 7 days",
    label: "7d",
    title: "Last 7 days",
    value: "7d",
  },
  {
    accessibleLabel: "Last 30 days",
    label: "30d",
    title: "Last 30 days",
    value: "30d",
  },
  {
    accessibleLabel: "Last 90 days",
    label: "90d",
    title: "Last 90 days",
    value: "90d",
  },
  {
    accessibleLabel: "Last 180 days",
    label: "180d",
    title: "Last 180 days",
    value: "180d",
  },
  {
    accessibleLabel: "Last year",
    label: "1y",
    title: "Last year",
    value: "1y",
  },
  {
    accessibleLabel: "All retained history",
    label: "All",
    title: "All retained history",
    value: "all",
  },
  {
    accessibleLabel: "Custom time range",
    label: "Custom",
    title: "Custom time range",
    value: "custom",
  },
];

export function MonitoringRangeTabs({
  ariaLabel,
  className = "",
  onChange,
  value,
}: {
  ariaLabel: string;
  className?: string;
  onChange: (value: MonitoringWindow) => void;
  value: MonitoringWindow;
}) {
  const tabsRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    tabsRef.current
      ?.querySelector<HTMLElement>(`[data-window="${value}"]`)
      ?.scrollIntoView({ block: "nearest", inline: "center" });
  }, [value]);

  return (
    <div
      aria-label={ariaLabel}
      className={`timeRangeTabs${className ? ` ${className}` : ""}`}
      ref={tabsRef}
      role="group"
    >
      {MONITORING_RANGE_OPTIONS.map((option) => (
        <button
          aria-label={option.accessibleLabel}
          aria-pressed={value === option.value}
          className={value === option.value ? "active" : ""}
          data-window={option.value}
          key={option.value}
          onClick={() => onChange(option.value)}
          title={option.title}
          type="button"
        >
          {option.label}
        </button>
      ))}
    </div>
  );
}
