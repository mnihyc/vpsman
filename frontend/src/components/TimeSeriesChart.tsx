import { useEffect, useMemo, useRef, useState } from "react";
import uPlot from "uplot";
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
  times: string[];
  valueFormatter: (value: number | null) => string;
};

export function TimeSeriesChart({
  ariaLabel,
  emptyLabel,
  height = 236,
  lines,
  times,
  valueFormatter,
}: TimeSeriesChartProps) {
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
          grid: { stroke: "#eef1f6", width: 1 },
          size: 34,
          stroke: "#5f6368",
          values: (_plot, ticks) => ticks.map((tick) => formatAxisTime(tick, unixTimes)),
        },
        {
          grid: { stroke: "#eef1f6", width: 1 },
          size: 78,
          stroke: "#5f6368",
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
          points: { show: true, size: 4, width: 1 },
          spanGaps: true,
          stroke: line.color,
          width: 2,
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
  }, [data, height, sanitizedLines, unixTimes, valueFormatter]);

  const hasData = unixTimes.length > 0 && sanitizedLines.length > 0;

  return (
    <div className="timeSeriesChartShell" role="img" aria-label={ariaLabel}>
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
        </>
      ) : (
        <div className="dashboardEmptyChart">{emptyLabel}</div>
      )}
    </div>
  );
}

function formatChartTime(unixTime: number): string {
  return new Date(unixTime * 1000).toLocaleString(undefined, {
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
    month: "short",
  });
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
