export const consolePalette = {
  accent: {
    primary: "#1a73e8",
    primaryText: "#174ea6",
    selected: "#eef4ff",
  },
  chart: {
    amber: "#8a4d00",
    blue: "#1a73e8",
    cyan: "#129eaf",
    green: "#188038",
    neutral: "#5f6368",
    orange: "#f29900",
    purple: "#9334e6",
    red: "#d93025",
  },
  neutral: {
    borderSubtle: "#eef1f6",
    muted: "#5f6368",
    text: "#202124",
    terminalForeground: "#eef1f6",
  },
} as const;

export const dashboardChartColors = [
  consolePalette.chart.blue,
  consolePalette.chart.green,
  consolePalette.chart.orange,
  consolePalette.chart.purple,
  consolePalette.chart.red,
  consolePalette.chart.cyan,
  consolePalette.chart.neutral,
  consolePalette.chart.amber,
] as const;

export const fleetChartColors = [
  consolePalette.chart.blue,
  consolePalette.chart.green,
  consolePalette.chart.orange,
  consolePalette.chart.purple,
  consolePalette.chart.red,
] as const;
