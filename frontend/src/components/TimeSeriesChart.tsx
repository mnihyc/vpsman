import { useEffect, useId, useMemo, useRef, useState } from "react";
import uPlot from "uplot";
import { consolePalette } from "../colorPalette";
import "uplot/dist/uPlot.min.css";

export type TimeSeriesChartLine = {
  color: string;
  label: string;
  values: Array<number | null>;
};

type HoverState = {
  index: number;
  timeLabel: string;
  values: Array<{ color: string; label: string; value: number | null }>;
};

type TimeSeriesChartProps = {
  ariaLabel: string;
  emptyLabel: string;
  height?: number;
  lines: TimeSeriesChartLine[];
  pointsOnly?: boolean;
  times: string[];
  valueFormatter: (value: number | null) => string;
};

export function TimeSeriesChart({
  ariaLabel,
  emptyLabel,
  height = 236,
  lines,
  pointsOnly = false,
  times,
  valueFormatter,
}: TimeSeriesChartProps) {
  const captionId = useId();
  const hostRef = useRef<HTMLDivElement | null>(null);
  const plotRef = useRef<uPlot | null>(null);
  const [hover, setHover] = useState<HoverState | null>(null);
  const unixTimes = useMemo(
    () =>
      times
        .map((time) => Math.floor(new Date(time).getTime() / 1000))
        .filter((time) => Number.isFinite(time)),
    [times],
  );
  const sanitizedLines = useMemo(
    () =>
      lines
        .map((line) => ({
          ...line,
          values: unixTimes.map((_, index) => line.values[index] ?? null),
        }))
        .filter((line) => line.values.some((value) => value !== null && Number.isFinite(value))),
    [lines, unixTimes],
  );
  const data = useMemo(
    () =>
      [
        unixTimes,
        ...sanitizedLines.map((line) => line.values.map((value) => (Number.isFinite(value) ? value : null))),
      ] as uPlot.AlignedData,
    [sanitizedLines, unixTimes],
  );

  useEffect(() => {
    const host = hostRef.current;
    if (!host || unixTimes.length === 0 || sanitizedLines.length === 0) {
      plotRef.current?.destroy();
      plotRef.current = null;
      setHover(null);
      return;
    }

    const buildOptions = (width: number): uPlot.Options => ({
      axes: [
        {
          grid: { stroke: consolePalette.neutral.borderSubtle, width: 1 },
          size: 34,
          stroke: consolePalette.neutral.muted,
          values: (_plot, ticks) =>
            formatAxisTicks(ticks, width, unixTimes),
        },
        {
          grid: { stroke: consolePalette.neutral.borderSubtle, width: 1 },
          size: 78,
          stroke: consolePalette.neutral.muted,
          values: (_plot, ticks) => ticks.map((tick) => valueFormatter(tick)),
        },
      ],
      cursor: {
        drag: { x: false, y: false },
        focus: { prox: 24 },
        points: { show: true, size: 6 },
      },
      height,
      hooks: {
        setCursor: [
          (plot) => {
            const index = plot.cursor.idx;
            if (index === null || index === undefined || index < 0 || index >= unixTimes.length) {
              setHover(null);
              return;
            }
            setHover({
              index,
              timeLabel: formatChartTime(unixTimes[index]),
              values: sanitizedLines.map((line) => ({
                color: line.color,
                label: line.label,
                value: line.values[index] ?? null,
              })),
            });
          },
        ],
      },
      legend: { show: false },
      padding: [8, 10, 0, 0],
      scales: {
        x: {
          range: (_plot, min, max) => {
            if (unixTimes.length === 1) {
              return [unixTimes[0] - 30 * 60, unixTimes[0] + 30 * 60];
            }
            return [min, max];
          },
          time: true,
        },
        y: { range: (_plot, min, max) => [Math.min(0, min), Math.max(1, max * 1.08)] },
      },
      series: [
        {},
        ...sanitizedLines.map((line) => ({
          label: line.label,
          points: { show: true, size: pointsOnly ? 6 : 4, width: 1 },
          spanGaps: false,
          stroke: line.color,
          width: pointsOnly ? 0 : 2,
        })),
      ],
      width,
    });

    const width = Math.max(320, host.clientWidth);
    const plot = new uPlot(buildOptions(width), data, host);
    plotRef.current = plot;
    const resizeObserver = new ResizeObserver((entries) => {
      const width = Math.max(320, Math.floor(entries[0]?.contentRect.width ?? host.clientWidth));
      plot.setSize({ height, width });
    });
    resizeObserver.observe(host);

    return () => {
      resizeObserver.disconnect();
      plot.destroy();
      plotRef.current = null;
    };
  }, [data, height, pointsOnly, sanitizedLines, unixTimes, valueFormatter]);

  const hasData = unixTimes.length > 0 && sanitizedLines.length > 0;
  const accessibleRows = useMemo(() => {
    const firstIndex = Math.max(0, unixTimes.length - 12);
    return unixTimes.slice(firstIndex).map((time, offset) => {
      const sourceIndex = firstIndex + offset;
      return {
        timeLabel: formatChartTime(time),
        values: sanitizedLines.map((line) => ({
          label: line.label,
          value: valueFormatter(line.values[sourceIndex] ?? null),
        })),
      };
    });
  }, [sanitizedLines, unixTimes, valueFormatter]);
  const latestValues = accessibleRows[accessibleRows.length - 1]?.values ?? [];
  const coverageLabel = useMemo(
    () => chartCoverageLabel(unixTimes, sanitizedLines),
    [sanitizedLines, unixTimes],
  );

  return (
    <figure
      className="timeSeriesChartShell"
      aria-labelledby={captionId}
      data-gap-policy="preserve"
      data-render-mode={pointsOnly ? "points" : "line"}
    >
      <figcaption className="srOnly" id={captionId}>
        {ariaLabel}
        {latestValues.length > 0
          ? `. Latest values: ${latestValues
              .map((entry) => `${entry.label} ${entry.value}`)
              .join(", ")}.`
          : "."}
      </figcaption>
      {hasData ? (
        <>
          <div className="timeSeriesChart" ref={hostRef} />
          <div className="timeSeriesLegend">
            {sanitizedLines.map((line) => (
              <span key={line.label}>
                <i style={{ background: line.color }} />
                {line.label}
              </span>
            ))}
          </div>
          {coverageLabel && (
            <p className="timeSeriesCoverage" aria-label={`${ariaLabel} data coverage`}>
              {coverageLabel}
            </p>
          )}
          {hover && (
            <div className="timeSeriesHover">
              <strong>{hover.timeLabel}</strong>
              {hover.values.map((entry) => (
                <span key={`${hover.index}-${entry.label}`}>
                  <i style={{ background: entry.color }} />
                  {entry.label}
                  <b>{valueFormatter(entry.value)}</b>
                </span>
              ))}
            </div>
          )}
          <table className="srOnly">
            <caption>{ariaLabel} data, latest {accessibleRows.length} points</caption>
            <thead>
              <tr>
                <th scope="col">Time</th>
                {sanitizedLines.map((line) => (
                  <th key={line.label} scope="col">
                    {line.label}
                  </th>
                ))}
              </tr>
            </thead>
            <tbody>
              {accessibleRows.map((row, index) => (
                <tr key={`${row.timeLabel}-${index}`}>
                  <th scope="row">{row.timeLabel}</th>
                  {row.values.map((entry) => (
                    <td key={`${row.timeLabel}-${entry.label}`}>{entry.value}</td>
                  ))}
                </tr>
              ))}
            </tbody>
          </table>
        </>
      ) : (
        <div className="dashboardEmptyChart">{emptyLabel}</div>
      )}
    </figure>
  );
}

function chartCoverageLabel(
  unixTimes: number[],
  lines: TimeSeriesChartLine[],
): string | null {
  const totalPoints = unixTimes.length * lines.length;
  if (totalPoints === 0) {
    return null;
  }

  let observedPoints = 0;
  let firstObservedIndex: number | null = null;
  let lastObservedIndex: number | null = null;
  for (const line of lines) {
    line.values.forEach((value, index) => {
      if (!Number.isFinite(value)) {
        return;
      }
      observedPoints += 1;
      firstObservedIndex =
        firstObservedIndex === null ? index : Math.min(firstObservedIndex, index);
      lastObservedIndex =
        lastObservedIndex === null ? index : Math.max(lastObservedIndex, index);
    });
  }

  if (observedPoints === 0 || firstObservedIndex === null || lastObservedIndex === null) {
    return null;
  }

  const missingPoints = totalPoints - observedPoints;
  const gapLabel =
    missingPoints === 0
      ? "no gaps"
      : `${missingPoints} ${missingPoints === 1 ? "gap" : "gaps"}`;
  return [
    `Data coverage: ${observedPoints}/${totalPoints} points present in selected range`,
    gapLabel,
    `samples ${formatChartTime(unixTimes[firstObservedIndex])} to ${formatChartTime(unixTimes[lastObservedIndex])}`,
  ].join(" · ");
}

function formatChartTime(unixTime: number): string {
  return new Date(unixTime * 1000).toLocaleString(undefined, {
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
    month: "short",
  });
}

function formatAxisTicks(
  ticks: number[],
  width: number,
  unixTimes: number[],
): string[] {
  const maxLabels =
    width < 420 ? 2 : width < 560 ? 3 : width < 760 ? 4 : ticks.length;
  if (ticks.length <= maxLabels) {
    return ticks.map((tick) => formatAxisTime(tick, unixTimes));
  }
  const visible = new Set<number>();
  for (let index = 0; index < maxLabels; index += 1) {
    visible.add(Math.round((index * (ticks.length - 1)) / (maxLabels - 1)));
  }
  return ticks.map((tick, index) =>
    visible.has(index) ? formatAxisTime(tick, unixTimes) : "",
  );
}

function formatAxisTime(unixTime: number, unixTimes: number[]): string {
  const first = unixTimes[0] ?? unixTime;
  const last = unixTimes[unixTimes.length - 1] ?? unixTime;
  const span = Math.max(60 * 60, last - first);
  const options: Intl.DateTimeFormatOptions =
    span > 48 * 60 * 60
      ? { day: "2-digit", month: "short" }
      : { hour: "2-digit", minute: "2-digit" };
  return new Date(unixTime * 1000).toLocaleString(undefined, options);
}
