import type { DispatchMode, SupervisorAction } from "./panels/jobDispatchModel";
import type {
  BrowserDownloadSinkMode,
  BrowserTransferMultiTargetPolicy,
} from "./resumableFileTransfer";
import type { FileExistingPolicy } from "./types";

export const DEFAULT_UPDATE_VERSION_URL =
  "https://github.com/mnihyc/vpsman/releases/latest/download/version.json";

export type JobDispatchPreset = {
  commandTemplateId?: string;
  requestId: string;
  mode: DispatchMode;
  selectorExpression?: string;
  maxTimeoutSecs?: number;
  fileFollowSymlinks?: boolean;
  filePath?: string;
  filePushMode?: string;
  filePushPath?: string;
  fileTransferChunkSize?: number;
  fileTransferDownloadName?: string;
  fileTransferDownloadSink?: BrowserDownloadSinkMode;
  fileTransferExistingPolicy?: FileExistingPolicy;
  fileTransferUploadFile?: File;
  fileTransferMultiTargetPolicy?: BrowserTransferMultiTargetPolicy;
  fileTransferRateLimit?: number;
  fileTransferResumeToken?: string;
  fileTransferSessionId?: string;
  fileTransferSourceArtifactId?: string;
  fileTransferUploadSourceKind?: "local-file" | "source-artifact";
  supervisorAction?: SupervisorAction;
  supervisorArgv?: string;
  supervisorCwd?: string;
  supervisorEnv?: string;
  supervisorLogBytes?: number;
  supervisorName?: string;
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
