import type { DispatchMode } from "./panels/jobDispatchModel";

export const DEFAULT_UPDATE_VERSION_URL =
  "https://github.com/mnihyc/vpsman/releases/latest/download/version.json";

export type JobDispatchPreset = {
  requestId: string;
  mode: DispatchMode;
  selectorExpression?: string;
  maxTimeoutSecs?: number;
  updateActivationSha256Hex?: string;
  updateArtifactUrl?: string;
  updateCheckActivate?: boolean;
  updateCheckRestartAgent?: boolean;
  updateCheckVersionUrl?: string;
  updateRestartAgent?: boolean;
  updateRollbackSha256Hex?: string;
  updateSha256Hex?: string;
};

export type JobDispatchPresetInput = Omit<JobDispatchPreset, "requestId">;
