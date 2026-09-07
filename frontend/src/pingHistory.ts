export type PingHistorySample = {
  sample_count: number;
  success_count: number;
  latency_avg_ms: number | null;
  loss_ratio: number;
};

export function summarizePingHistory(rows: readonly PingHistorySample[]): {
  latency: number | null;
  loss: number | null;
} {
  let samples = 0;
  let loss = 0;
  let successes = 0;
  let latency = 0;
  for (const row of rows) {
    samples += row.sample_count;
    loss += row.loss_ratio * row.sample_count;
    if (row.latency_avg_ms !== null) {
      successes += row.success_count;
      latency += row.latency_avg_ms * row.success_count;
    }
  }
  return {
    latency: successes > 0 ? latency / successes : null,
    loss: samples > 0 ? loss / samples : null,
  };
}

export function pingLossWindowSecs(stepSecs: number): number {
  const step = Math.max(60, stepSecs);
  return Math.ceil(300 / step) * step;
}

// The caller supplies equal-spaced chart buckets, including null gaps. Average
// a five-minute target window (rounded up to whole buckets) of available samples;
// neither cross a gap nor fetch evidence before the selected range.
export function smoothPingLoss(
  rows: readonly (PingHistorySample | null)[],
  stepSecs: number,
): Array<number | null> {
  const windowPoints = pingLossWindowSecs(stepSecs) / Math.max(60, stepSecs);
  if (windowPoints === 1) return rows.map((row) => row?.loss_ratio ?? null);

  let start = 0;
  let samples = 0;
  let loss = 0;
  return rows.map((row, index) => {
    if (row === null) {
      start = index + 1;
      samples = 0;
      loss = 0;
      return null;
    }
    samples += row.sample_count;
    loss += row.loss_ratio * row.sample_count;
    if (index - start >= windowPoints) {
      const expired = rows[start++]!;
      samples -= expired.sample_count;
      loss -= expired.loss_ratio * expired.sample_count;
    }
    // Sliding subtraction can leave an IEEE-754 residual below zero or above
    // one. Preserve the ratio's mathematical bounds without an epsilon cutoff.
    return samples > 0 ? Math.min(1, Math.max(0, loss / samples)) : null;
  });
}
