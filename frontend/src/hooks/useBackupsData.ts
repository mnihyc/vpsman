import { useCallback, useRef, useState } from "react";
import {
  apiGet,
  apiGetBlob,
  apiPost,
  apiPut,
  buildListPath,
  isApiUnauthorized,
  LatestReadConsumer,
} from "../api";
import { bytesToBase64, readFileSlice, sha256FileHex } from "../fileTransfer";
import { FLEET_DETAIL_LIMIT, HISTORY_DETAIL_LIMIT } from "../constants";
import {
  snapshotSourceAvailable,
  snapshotSourceError,
  type SnapshotSource,
} from "../homeSnapshot";
import type {
  BackupArtifactRecord,
  BackupArtifactHandoffRecord,
  BackupArtifactHandoffRequest,
  BackupArtifactUploadSessionRecord,
  BackupPolicyPruneRequest,
  BackupPolicyPruneResponse,
  BackupPolicyRecord,
  BackupRequestRecord,
  CreateBackupPolicyRequest,
  CreateBackupRequest,
  CreateMigrationLinkRequest,
  CreateMigrationRunRequest,
  CreateMigrationRunResponse,
  CreateRestorePlanRequest,
  MigrationLinkRecord,
  RestorePlanRecord,
  UpdateBackupPolicyRequest,
  UploadBackupArtifactRequest,
} from "../types";

const BACKUP_ARTIFACT_UPLOAD_CHUNK_BYTES = 4 * 1024 * 1024;

export type BackupProjection =
  | "artifacts"
  | "migrationLinks"
  | "policies"
  | "requests"
  | "restorePlans";

type BackupProjectionCounters = Record<BackupProjection, number>;
type BackupProjectionFailures = Record<BackupProjection, string | null>;

function emptyBackupProjectionCounters(): BackupProjectionCounters {
  return {
    artifacts: 0,
    migrationLinks: 0,
    policies: 0,
    requests: 0,
    restorePlans: 0,
  };
}

function emptyBackupProjectionFailures(): BackupProjectionFailures {
  return {
    artifacts: null,
    migrationLinks: null,
    policies: null,
    requests: null,
    restorePlans: null,
  };
}

function formatBackupProjectionFailures(
  failures: BackupProjectionFailures,
): string | null {
  const current = [
    failures.requests,
    failures.policies,
    failures.artifacts,
    failures.restorePlans,
    failures.migrationLinks,
  ].filter((message): message is string => message !== null);
  return current.length > 0 ? current.join("; ") : null;
}

function settledSourceFailure(
  label: string,
  result: PromiseSettledResult<unknown>,
): string | null {
  if (result.status === "fulfilled") {
    return null;
  }
  return `${label}: ${
    result.reason instanceof Error
      ? result.reason.message
      : "source unavailable"
  }`;
}

export function useBackupsData(
  apiToken: string,
  onUnauthorized: () => void,
  onAuditChanged: () => Promise<void>,
) {
  const apiTokenRef = useRef(apiToken);
  apiTokenRef.current = apiToken;
  const [backups, setBackups] = useState<BackupRequestRecord[]>([]);
  const [backupPolicies, setBackupPolicies] = useState<BackupPolicyRecord[]>(
    [],
  );
  const [backupPoliciesTruncated, setBackupPoliciesTruncated] = useState(false);
  const [backupArtifacts, setBackupArtifacts] = useState<
    BackupArtifactRecord[]
  >([]);
  const [backupsTruncated, setBackupsTruncated] = useState(false);
  const [backupArtifactsTruncated, setBackupArtifactsTruncated] =
    useState(false);
  const [restorePlans, setRestorePlans] = useState<RestorePlanRecord[]>([]);
  const [migrationLinks, setMigrationLinks] = useState<MigrationLinkRecord[]>(
    [],
  );
  const [backupsError, setBackupsError] = useState<string | null>(null);
  const [backupsLoading, setBackupsLoading] = useState(false);
  const [backupSourceLoadingVersion, setBackupSourceLoadingVersion] =
    useState(0);
  const [backupsEvidenceAvailable, setBackupsEvidenceAvailable] =
    useState(false);
  // The operation fence owns only the shared loading indicator. Revisions and
  // consumers are projection-local so an exact artifact read or mutation does
  // not discard an aggregate read's unrelated sources.
  const backupsLoadOperationGeneration = useRef(0);
  const backupProjectionGenerationsRef = useRef<BackupProjectionCounters>(
    emptyBackupProjectionCounters(),
  );
  const backupLoadConsumersRef = useRef({
    artifacts: new LatestReadConsumer<BackupArtifactRecord[]>(),
    migrationLinks: new LatestReadConsumer<MigrationLinkRecord[]>(),
    policies: new LatestReadConsumer<BackupPolicyRecord[]>(),
    requests: new LatestReadConsumer<BackupRequestRecord[]>(),
    restorePlans: new LatestReadConsumer<RestorePlanRecord[]>(),
  });
  const homeBackupsHydrationRef = useRef<{
    artifactGeneration: number;
    operationGeneration: number;
    requestGeneration: number;
  } | null>(null);
  const backupProjectionFailuresRef = useRef<BackupProjectionFailures>(
    emptyBackupProjectionFailures(),
  );
  const backupSourceLoadTokens = useRef(
    new Map<BackupProjection, Set<number>>(),
  );
  const nextBackupSourceLoadToken = useRef(0);
  const backupRequestsEvidenceAvailableRef = useRef(false);
  const backupArtifactsEvidenceAvailableRef = useRef(false);
  const backupsRef = useRef(backups);
  const backupPoliciesRef = useRef(backupPolicies);
  const backupArtifactsRef = useRef(backupArtifacts);
  const restorePlansRef = useRef(restorePlans);
  const migrationLinksRef = useRef(migrationLinks);
  backupsRef.current = backups;
  backupPoliciesRef.current = backupPolicies;
  backupArtifactsRef.current = backupArtifacts;
  restorePlansRef.current = restorePlans;
  migrationLinksRef.current = migrationLinks;

  const publishBackupsEvidence = useCallback(() => {
    setBackupsEvidenceAvailable(
      backupRequestsEvidenceAvailableRef.current &&
        backupArtifactsEvidenceAvailableRef.current,
    );
  }, []);

  const publishBackupsError = useCallback(() => {
    setBackupsError(
      formatBackupProjectionFailures(backupProjectionFailuresRef.current),
    );
  }, []);

  const trackBackupSourceLoad = useCallback(
    async <T,>(source: BackupProjection, load: () => Promise<T>): Promise<T> => {
      const token = ++nextBackupSourceLoadToken.current;
      const sourceTokens = backupSourceLoadTokens.current.get(source) ?? new Set();
      sourceTokens.add(token);
      backupSourceLoadTokens.current.set(source, sourceTokens);
      setBackupSourceLoadingVersion((version) => version + 1);
      try {
        return await load();
      } finally {
        const currentTokens = backupSourceLoadTokens.current.get(source);
        currentTokens?.delete(token);
        if (currentTokens?.size === 0) backupSourceLoadTokens.current.delete(source);
        setBackupSourceLoadingVersion((version) => version + 1);
      }
    },
    [],
  );

  const backupSourcesLoading = useCallback(
    (sources: readonly BackupProjection[]) => {
      void backupSourceLoadingVersion;
      return sources.some(
        (source) => (backupSourceLoadTokens.current.get(source)?.size ?? 0) > 0,
      );
    },
    [backupSourceLoadingVersion],
  );

  const backupSourcesError = useCallback(
    (sources: readonly BackupProjection[]) => {
      if (backupsError === "Operator login required") return backupsError;
      void backupsError;
      const errors = sources.flatMap((source) => {
        const error = backupProjectionFailuresRef.current[source];
        return error ? [error] : [];
      });
      return errors.length > 0 ? errors.join("; ") : null;
    },
    [backupsError],
  );

  const handleBackupsUnauthorized = useCallback(() => {
    backupsLoadOperationGeneration.current += 1;
    const generations = backupProjectionGenerationsRef.current;
    generations.artifacts += 1;
    generations.migrationLinks += 1;
    generations.policies += 1;
    generations.requests += 1;
    generations.restorePlans += 1;
    backupLoadConsumersRef.current.artifacts.discardPending([]);
    backupLoadConsumersRef.current.migrationLinks.discardPending([]);
    backupLoadConsumersRef.current.policies.discardPending([]);
    backupLoadConsumersRef.current.requests.discardPending([]);
    backupLoadConsumersRef.current.restorePlans.discardPending([]);
    homeBackupsHydrationRef.current = null;
    backupRequestsEvidenceAvailableRef.current = false;
    backupArtifactsEvidenceAvailableRef.current = false;
    backupProjectionFailuresRef.current = emptyBackupProjectionFailures();
    backupSourceLoadTokens.current.clear();
    setBackupSourceLoadingVersion((version) => version + 1);
    setBackupsEvidenceAvailable(false);
    backupsRef.current = [];
    setBackups([]);
    setBackupsTruncated(false);
    backupPoliciesRef.current = [];
    setBackupPolicies([]);
    setBackupPoliciesTruncated(false);
    backupArtifactsRef.current = [];
    setBackupArtifacts([]);
    setBackupArtifactsTruncated(false);
    restorePlansRef.current = [];
    setRestorePlans([]);
    migrationLinksRef.current = [];
    setMigrationLinks([]);
    setBackupsError("Operator login required");
    setBackupsLoading(false);
    apiTokenRef.current = "";
    onUnauthorized();
  }, [onUnauthorized]);

  const loadBackups = useCallback((): Promise<void> => {
    if (apiTokenRef.current !== apiToken) {
      return Promise.resolve();
    }
    const operationGeneration = ++backupsLoadOperationGeneration.current;
    const generations = backupProjectionGenerationsRef.current;
    const requestGeneration = ++generations.requests;
    const policyGeneration = ++generations.policies;
    const artifactGeneration = ++generations.artifacts;
    const restorePlanGeneration = ++generations.restorePlans;
    const migrationLinkGeneration = ++generations.migrationLinks;
    backupProjectionFailuresRef.current = emptyBackupProjectionFailures();
    setBackupsLoading(true);
    setBackupsError(null);
    return (async () => {
      try {
        const results = await Promise.allSettled([
          backupLoadConsumersRef.current.requests.enqueue(() =>
            apiGet<BackupRequestRecord[]>(
              buildListPath("/api/v1/backups", {
                limit: HISTORY_DETAIL_LIMIT,
                sort: "created_at",
                dir: "desc",
              }),
              apiToken,
            ),
          ),
          backupLoadConsumersRef.current.policies.enqueue(() =>
            apiGet<BackupPolicyRecord[]>(
              `/api/v1/backup-policies?limit=${FLEET_DETAIL_LIMIT}`,
              apiToken,
            ),
          ),
          backupLoadConsumersRef.current.artifacts.enqueue(() =>
            apiGet<BackupArtifactRecord[]>(
              buildListPath("/api/v1/backup-artifacts", {
                limit: HISTORY_DETAIL_LIMIT,
                sort: "created_at",
                dir: "desc",
              }),
              apiToken,
            ),
          ),
          backupLoadConsumersRef.current.restorePlans.enqueue(() =>
            apiGet<RestorePlanRecord[]>(
              buildListPath("/api/v1/restore-plans", {
                limit: HISTORY_DETAIL_LIMIT,
                sort: "created_at",
                dir: "desc",
              }),
              apiToken,
            ),
          ),
          backupLoadConsumersRef.current.migrationLinks.enqueue(() =>
            apiGet<MigrationLinkRecord[]>(
              buildListPath("/api/v1/migration-links", {
                limit: HISTORY_DETAIL_LIMIT,
                sort: "created_at",
                dir: "desc",
              }),
              apiToken,
            ),
          ),
        ]);
        if (apiTokenRef.current !== apiToken) {
          return;
        }
        const currentGenerations = backupProjectionGenerationsRef.current;
        const sourceIsCurrent = [
          currentGenerations.requests === requestGeneration,
          currentGenerations.policies === policyGeneration,
          currentGenerations.artifacts === artifactGeneration,
          currentGenerations.restorePlans === restorePlanGeneration,
          currentGenerations.migrationLinks === migrationLinkGeneration,
        ] as const;
        const unauthorized = results.some(
          (result, index) =>
            sourceIsCurrent[index] &&
            result.status === "rejected" &&
            isApiUnauthorized(result.reason),
        );
        if (unauthorized) {
          handleBackupsUnauthorized();
          return;
        }
        const [
          backupResult,
          policyResult,
          artifactResult,
          restoreResult,
          migrationResult,
        ] = results;
        if (sourceIsCurrent[0]) {
          backupRequestsEvidenceAvailableRef.current =
            backupResult.status === "fulfilled";
          backupProjectionFailuresRef.current.requests = settledSourceFailure(
            "Backup requests",
            backupResult,
          );
          if (backupResult.status === "fulfilled") {
            backupsRef.current = backupResult.value;
            setBackups(backupResult.value);
            setBackupsTruncated(
              backupResult.value.length >= HISTORY_DETAIL_LIMIT,
            );
          }
        }
        if (sourceIsCurrent[1]) {
          backupProjectionFailuresRef.current.policies = settledSourceFailure(
            "Backup policies",
            policyResult,
          );
          if (policyResult.status === "fulfilled") {
            backupPoliciesRef.current = policyResult.value;
            setBackupPolicies(policyResult.value);
            setBackupPoliciesTruncated(
              policyResult.value.length >= FLEET_DETAIL_LIMIT,
            );
          }
        }
        if (sourceIsCurrent[2]) {
          backupArtifactsEvidenceAvailableRef.current =
            artifactResult.status === "fulfilled";
          backupProjectionFailuresRef.current.artifacts = settledSourceFailure(
            "Backup artifacts",
            artifactResult,
          );
          if (artifactResult.status === "fulfilled") {
            backupArtifactsRef.current = artifactResult.value;
            setBackupArtifacts(artifactResult.value);
            setBackupArtifactsTruncated(
              artifactResult.value.length >= HISTORY_DETAIL_LIMIT,
            );
          }
        }
        if (sourceIsCurrent[3]) {
          backupProjectionFailuresRef.current.restorePlans =
            settledSourceFailure("Restore plans", restoreResult);
          if (restoreResult.status === "fulfilled") {
            restorePlansRef.current = restoreResult.value;
            setRestorePlans(restoreResult.value);
          }
        }
        if (sourceIsCurrent[4]) {
          backupProjectionFailuresRef.current.migrationLinks =
            settledSourceFailure("Migration links", migrationResult);
          if (migrationResult.status === "fulfilled") {
            migrationLinksRef.current = migrationResult.value;
            setMigrationLinks(migrationResult.value);
          }
        }
        publishBackupsEvidence();
        publishBackupsError();
      } finally {
        if (
          backupsLoadOperationGeneration.current === operationGeneration &&
          apiTokenRef.current === apiToken
        ) {
          setBackupsLoading(false);
        }
      }
    })();
  }, [
    apiToken,
    handleBackupsUnauthorized,
    publishBackupsError,
    publishBackupsEvidence,
  ]);

  const beginHomeBackupsHydration = useCallback(() => {
    const operationGeneration = ++backupsLoadOperationGeneration.current;
    const generations = backupProjectionGenerationsRef.current;
    homeBackupsHydrationRef.current = {
      artifactGeneration: ++generations.artifacts,
      operationGeneration,
      requestGeneration: ++generations.requests,
    };
    setBackupsLoading(true);
    return operationGeneration;
  }, []);

  const hydrateHomeBackups = useCallback(
    (
      generation: number,
      backupSource: SnapshotSource<BackupRequestRecord[]>,
      artifactSource: SnapshotSource<BackupArtifactRecord[]>,
    ) => {
      if (apiTokenRef.current !== apiToken) {
        return;
      }
      const hydration = homeBackupsHydrationRef.current;
      if (hydration?.operationGeneration !== generation) {
        return;
      }
      const currentGenerations = backupProjectionGenerationsRef.current;
      const requestIsCurrent =
        currentGenerations.requests === hydration.requestGeneration;
      const artifactIsCurrent =
        currentGenerations.artifacts === hydration.artifactGeneration;
      if (requestIsCurrent) {
        if (snapshotSourceAvailable(backupSource)) {
          backupsRef.current = backupSource.data;
          setBackups(backupSource.data);
          setBackupsTruncated(backupSource.data.length >= HISTORY_DETAIL_LIMIT);
        }
        backupRequestsEvidenceAvailableRef.current =
          snapshotSourceAvailable(backupSource);
        backupProjectionFailuresRef.current.requests = snapshotSourceError(
          "Backup requests",
          backupSource,
        );
      }
      if (artifactIsCurrent) {
        if (snapshotSourceAvailable(artifactSource)) {
          backupArtifactsRef.current = artifactSource.data;
          setBackupArtifacts(artifactSource.data);
          setBackupArtifactsTruncated(
            artifactSource.data.length >= HISTORY_DETAIL_LIMIT,
          );
        }
        backupArtifactsEvidenceAvailableRef.current =
          snapshotSourceAvailable(artifactSource);
        backupProjectionFailuresRef.current.artifacts = snapshotSourceError(
          "Backup artifacts",
          artifactSource,
        );
      }
      if (requestIsCurrent || artifactIsCurrent) {
        publishBackupsEvidence();
        publishBackupsError();
      }
      if (backupsLoadOperationGeneration.current === generation) {
        setBackupsLoading(false);
      }
    },
    [apiToken, publishBackupsError, publishBackupsEvidence],
  );

  const storeBackupRequest = useCallback(
    (record: BackupRequestRecord) => {
      backupProjectionGenerationsRef.current.requests += 1;
      backupProjectionFailuresRef.current.requests = null;
      const next = upsertBoundedRecord(
        backupsRef.current,
        record,
        (candidate) => candidate.id,
        HISTORY_DETAIL_LIMIT,
      );
      backupsRef.current = next;
      setBackups(next);
      setBackupsTruncated(
        (truncated) => truncated || next.length >= HISTORY_DETAIL_LIMIT,
      );
      publishBackupsError();
    },
    [publishBackupsError],
  );

  const storeBackupPolicy = useCallback(
    (record: BackupPolicyRecord) => {
      backupProjectionGenerationsRef.current.policies += 1;
      backupProjectionFailuresRef.current.policies = null;
      const next = upsertBoundedRecord(
        backupPoliciesRef.current,
        record,
        (candidate) => candidate.schedule_id,
        FLEET_DETAIL_LIMIT,
      );
      backupPoliciesRef.current = next;
      setBackupPolicies(next);
      setBackupPoliciesTruncated(
        (truncated) => truncated || next.length >= FLEET_DETAIL_LIMIT,
      );
      publishBackupsError();
    },
    [publishBackupsError],
  );

  const storeBackupArtifact = useCallback(
    (record: BackupArtifactRecord) => {
      backupProjectionGenerationsRef.current.artifacts += 1;
      backupProjectionFailuresRef.current.artifacts = null;
      const next = upsertBoundedRecord(
        backupArtifactsRef.current,
        record,
        (candidate) => candidate.id,
        HISTORY_DETAIL_LIMIT,
      );
      backupArtifactsRef.current = next;
      setBackupArtifacts(next);
      setBackupArtifactsTruncated(
        (truncated) => truncated || next.length >= HISTORY_DETAIL_LIMIT,
      );
      publishBackupsError();
    },
    [publishBackupsError],
  );

  const storeRestorePlan = useCallback(
    (record: RestorePlanRecord) => {
      backupProjectionGenerationsRef.current.restorePlans += 1;
      backupProjectionFailuresRef.current.restorePlans = null;
      const next = upsertBoundedRecord(
        restorePlansRef.current,
        record,
        (candidate) => candidate.id,
        HISTORY_DETAIL_LIMIT,
      );
      restorePlansRef.current = next;
      setRestorePlans(next);
      publishBackupsError();
    },
    [publishBackupsError],
  );

  const storeMigrationLink = useCallback(
    (record: MigrationLinkRecord) => {
      backupProjectionGenerationsRef.current.migrationLinks += 1;
      backupProjectionFailuresRef.current.migrationLinks = null;
      const next = upsertBoundedRecord(
        migrationLinksRef.current,
        record,
        (candidate) => candidate.id,
        HISTORY_DETAIL_LIMIT,
      );
      migrationLinksRef.current = next;
      setMigrationLinks(next);
      publishBackupsError();
    },
    [publishBackupsError],
  );

  const loadBackupRequests = useCallback((): Promise<void> => {
    if (apiTokenRef.current !== apiToken) {
      return Promise.resolve();
    }
    const operationGeneration = ++backupsLoadOperationGeneration.current;
    const requestGeneration = ++backupProjectionGenerationsRef.current.requests;
    backupProjectionFailuresRef.current.requests = null;
    setBackupsLoading(true);
    publishBackupsError();
    return (async () => {
      try {
        const records = await backupLoadConsumersRef.current.requests.enqueue(
          () =>
            apiGet<BackupRequestRecord[]>(
              buildListPath("/api/v1/backups", {
                limit: HISTORY_DETAIL_LIMIT,
                sort: "created_at",
                dir: "desc",
              }),
              apiToken,
            ),
        );
        if (
          apiTokenRef.current !== apiToken ||
          backupProjectionGenerationsRef.current.requests !== requestGeneration
        ) {
          return;
        }
        backupsRef.current = records;
        setBackups(records);
        setBackupsTruncated(records.length >= HISTORY_DETAIL_LIMIT);
        backupRequestsEvidenceAvailableRef.current = true;
        backupProjectionFailuresRef.current.requests = null;
        publishBackupsEvidence();
        publishBackupsError();
      } catch (error) {
        if (
          apiTokenRef.current !== apiToken ||
          backupProjectionGenerationsRef.current.requests !== requestGeneration
        ) {
          return;
        }
        if (isApiUnauthorized(error)) {
          handleBackupsUnauthorized();
        } else {
          backupRequestsEvidenceAvailableRef.current = false;
          backupProjectionFailuresRef.current.requests = settledSourceFailure(
            "Backup requests",
            { status: "rejected", reason: error },
          );
          publishBackupsEvidence();
          publishBackupsError();
        }
      } finally {
        if (
          apiTokenRef.current === apiToken &&
          backupsLoadOperationGeneration.current === operationGeneration
        ) {
          setBackupsLoading(false);
        }
      }
    })();
  }, [
    apiToken,
    handleBackupsUnauthorized,
    publishBackupsError,
    publishBackupsEvidence,
  ]);

  const loadBackupArtifacts = useCallback((): Promise<void> => {
    if (apiTokenRef.current !== apiToken) {
      return Promise.resolve();
    }
    const operationGeneration = ++backupsLoadOperationGeneration.current;
    const artifactGeneration = ++backupProjectionGenerationsRef.current
      .artifacts;
    backupProjectionFailuresRef.current.artifacts = null;
    setBackupsLoading(true);
    publishBackupsError();
    return (async () => {
      try {
        const records = await backupLoadConsumersRef.current.artifacts.enqueue(
          () =>
            apiGet<BackupArtifactRecord[]>(
              buildListPath("/api/v1/backup-artifacts", {
                limit: HISTORY_DETAIL_LIMIT,
                sort: "created_at",
                dir: "desc",
              }),
              apiToken,
            ),
        );
        if (
          apiTokenRef.current !== apiToken ||
          backupProjectionGenerationsRef.current.artifacts !==
            artifactGeneration
        ) {
          return;
        }
        backupArtifactsRef.current = records;
        setBackupArtifacts(records);
        setBackupArtifactsTruncated(records.length >= HISTORY_DETAIL_LIMIT);
        backupArtifactsEvidenceAvailableRef.current = true;
        backupProjectionFailuresRef.current.artifacts = null;
        publishBackupsEvidence();
        publishBackupsError();
      } catch (error) {
        if (
          apiTokenRef.current !== apiToken ||
          backupProjectionGenerationsRef.current.artifacts !==
            artifactGeneration
        ) {
          return;
        }
        if (isApiUnauthorized(error)) {
          handleBackupsUnauthorized();
        } else {
          backupArtifactsEvidenceAvailableRef.current = false;
          backupProjectionFailuresRef.current.artifacts = settledSourceFailure(
            "Backup artifacts",
            {
              status: "rejected",
              reason: error,
            },
          );
          publishBackupsEvidence();
          publishBackupsError();
        }
      } finally {
        if (
          apiTokenRef.current === apiToken &&
          backupsLoadOperationGeneration.current === operationGeneration
        ) {
          setBackupsLoading(false);
        }
      }
    })();
  }, [
    apiToken,
    handleBackupsUnauthorized,
    publishBackupsError,
    publishBackupsEvidence,
  ]);

  const loadBackupPolicies = useCallback((): Promise<void> => {
    if (apiTokenRef.current !== apiToken) return Promise.resolve();
    const operationGeneration = ++backupsLoadOperationGeneration.current;
    const generation = ++backupProjectionGenerationsRef.current.policies;
    backupProjectionFailuresRef.current.policies = null;
    setBackupsLoading(true);
    publishBackupsError();
    return (async () => {
      try {
        const records = await backupLoadConsumersRef.current.policies.enqueue(
          () =>
            apiGet<BackupPolicyRecord[]>(
              `/api/v1/backup-policies?limit=${FLEET_DETAIL_LIMIT}`,
              apiToken,
            ),
        );
        if (
          apiTokenRef.current !== apiToken ||
          backupProjectionGenerationsRef.current.policies !== generation
        ) {
          return;
        }
        backupPoliciesRef.current = records;
        setBackupPolicies(records);
        setBackupPoliciesTruncated(records.length >= FLEET_DETAIL_LIMIT);
        backupProjectionFailuresRef.current.policies = null;
        publishBackupsError();
      } catch (error) {
        if (
          apiTokenRef.current !== apiToken ||
          backupProjectionGenerationsRef.current.policies !== generation
        ) {
          return;
        }
        if (isApiUnauthorized(error)) handleBackupsUnauthorized();
        else {
          backupProjectionFailuresRef.current.policies = settledSourceFailure(
            "Backup policies",
            { status: "rejected", reason: error },
          );
          publishBackupsError();
        }
      } finally {
        if (
          apiTokenRef.current === apiToken &&
          backupsLoadOperationGeneration.current === operationGeneration
        ) {
          setBackupsLoading(false);
        }
      }
    })();
  }, [
    apiToken,
    handleBackupsUnauthorized,
    publishBackupsError,
  ]);

  const loadRestorePlans = useCallback((): Promise<void> => {
    if (apiTokenRef.current !== apiToken) return Promise.resolve();
    const operationGeneration = ++backupsLoadOperationGeneration.current;
    const generation = ++backupProjectionGenerationsRef.current.restorePlans;
    backupProjectionFailuresRef.current.restorePlans = null;
    setBackupsLoading(true);
    publishBackupsError();
    return (async () => {
      try {
        const records = await backupLoadConsumersRef.current.restorePlans.enqueue(
          () =>
            apiGet<RestorePlanRecord[]>(
              buildListPath("/api/v1/restore-plans", {
                limit: HISTORY_DETAIL_LIMIT,
                sort: "created_at",
                dir: "desc",
              }),
              apiToken,
            ),
        );
        if (
          apiTokenRef.current !== apiToken ||
          backupProjectionGenerationsRef.current.restorePlans !== generation
        ) {
          return;
        }
        restorePlansRef.current = records;
        setRestorePlans(records);
        backupProjectionFailuresRef.current.restorePlans = null;
        publishBackupsError();
      } catch (error) {
        if (
          apiTokenRef.current !== apiToken ||
          backupProjectionGenerationsRef.current.restorePlans !== generation
        ) {
          return;
        }
        if (isApiUnauthorized(error)) handleBackupsUnauthorized();
        else {
          backupProjectionFailuresRef.current.restorePlans = settledSourceFailure(
            "Restore plans",
            { status: "rejected", reason: error },
          );
          publishBackupsError();
        }
      } finally {
        if (
          apiTokenRef.current === apiToken &&
          backupsLoadOperationGeneration.current === operationGeneration
        ) {
          setBackupsLoading(false);
        }
      }
    })();
  }, [apiToken, handleBackupsUnauthorized, publishBackupsError]);

  const loadMigrationLinks = useCallback((): Promise<void> => {
    if (apiTokenRef.current !== apiToken) return Promise.resolve();
    const operationGeneration = ++backupsLoadOperationGeneration.current;
    const generation = ++backupProjectionGenerationsRef.current.migrationLinks;
    backupProjectionFailuresRef.current.migrationLinks = null;
    setBackupsLoading(true);
    publishBackupsError();
    return (async () => {
      try {
        const records = await backupLoadConsumersRef.current.migrationLinks.enqueue(
          () =>
            apiGet<MigrationLinkRecord[]>(
              buildListPath("/api/v1/migration-links", {
                limit: HISTORY_DETAIL_LIMIT,
                sort: "created_at",
                dir: "desc",
              }),
              apiToken,
            ),
        );
        if (
          apiTokenRef.current !== apiToken ||
          backupProjectionGenerationsRef.current.migrationLinks !== generation
        ) {
          return;
        }
        migrationLinksRef.current = records;
        setMigrationLinks(records);
        backupProjectionFailuresRef.current.migrationLinks = null;
        publishBackupsError();
      } catch (error) {
        if (
          apiTokenRef.current !== apiToken ||
          backupProjectionGenerationsRef.current.migrationLinks !== generation
        ) {
          return;
        }
        if (isApiUnauthorized(error)) handleBackupsUnauthorized();
        else {
          backupProjectionFailuresRef.current.migrationLinks = settledSourceFailure(
            "Migration links",
            { status: "rejected", reason: error },
          );
          publishBackupsError();
        }
      } finally {
        if (
          apiTokenRef.current === apiToken &&
          backupsLoadOperationGeneration.current === operationGeneration
        ) {
          setBackupsLoading(false);
        }
      }
    })();
  }, [apiToken, handleBackupsUnauthorized, publishBackupsError]);

  const loadBackupRequestArtifactProjections =
    useCallback((): Promise<void> => {
      if (apiTokenRef.current !== apiToken) {
        return Promise.resolve();
      }
      const operationGeneration = ++backupsLoadOperationGeneration.current;
      const generations = backupProjectionGenerationsRef.current;
      const requestGeneration = ++generations.requests;
      const artifactGeneration = ++generations.artifacts;
      backupProjectionFailuresRef.current.requests = null;
      backupProjectionFailuresRef.current.artifacts = null;
      setBackupsLoading(true);
      publishBackupsError();
      return (async () => {
        try {
          const [requestsResult, artifactsResult] = await Promise.allSettled([
            backupLoadConsumersRef.current.requests.enqueue(() =>
              apiGet<BackupRequestRecord[]>(
                buildListPath("/api/v1/backups", {
                  limit: HISTORY_DETAIL_LIMIT,
                  sort: "created_at",
                  dir: "desc",
                }),
                apiToken,
              ),
            ),
            backupLoadConsumersRef.current.artifacts.enqueue(() =>
              apiGet<BackupArtifactRecord[]>(
                buildListPath("/api/v1/backup-artifacts", {
                  limit: HISTORY_DETAIL_LIMIT,
                  sort: "created_at",
                  dir: "desc",
                }),
                apiToken,
              ),
            ),
          ]);
          if (apiTokenRef.current !== apiToken) {
            return;
          }
          const requestsAreCurrent =
            backupProjectionGenerationsRef.current.requests ===
            requestGeneration;
          const artifactsAreCurrent =
            backupProjectionGenerationsRef.current.artifacts ===
            artifactGeneration;
          if (
            (requestsAreCurrent &&
              requestsResult.status === "rejected" &&
              isApiUnauthorized(requestsResult.reason)) ||
            (artifactsAreCurrent &&
              artifactsResult.status === "rejected" &&
              isApiUnauthorized(artifactsResult.reason))
          ) {
            handleBackupsUnauthorized();
            return;
          }
          if (requestsAreCurrent) {
            backupRequestsEvidenceAvailableRef.current =
              requestsResult.status === "fulfilled";
            backupProjectionFailuresRef.current.requests = settledSourceFailure(
              "Backup requests",
              requestsResult,
            );
            if (requestsResult.status === "fulfilled") {
              backupsRef.current = requestsResult.value;
              setBackups(requestsResult.value);
              setBackupsTruncated(
                requestsResult.value.length >= HISTORY_DETAIL_LIMIT,
              );
            }
          }
          if (artifactsAreCurrent) {
            backupArtifactsEvidenceAvailableRef.current =
              artifactsResult.status === "fulfilled";
            backupProjectionFailuresRef.current.artifacts =
              settledSourceFailure("Backup artifacts", artifactsResult);
            if (artifactsResult.status === "fulfilled") {
              backupArtifactsRef.current = artifactsResult.value;
              setBackupArtifacts(artifactsResult.value);
              setBackupArtifactsTruncated(
                artifactsResult.value.length >= HISTORY_DETAIL_LIMIT,
              );
            }
          }
          publishBackupsEvidence();
          publishBackupsError();
        } finally {
          if (
            apiTokenRef.current === apiToken &&
            backupsLoadOperationGeneration.current === operationGeneration
          ) {
            setBackupsLoading(false);
          }
        }
      })();
    }, [
      apiToken,
      handleBackupsUnauthorized,
      publishBackupsError,
      publishBackupsEvidence,
    ]);

  const trackedLoadBackupRequests = useCallback(
    () => trackBackupSourceLoad("requests", loadBackupRequests),
    [loadBackupRequests, trackBackupSourceLoad],
  );
  const trackedLoadBackupPolicies = useCallback(
    () => trackBackupSourceLoad("policies", loadBackupPolicies),
    [loadBackupPolicies, trackBackupSourceLoad],
  );
  const trackedLoadBackupArtifacts = useCallback(
    () => trackBackupSourceLoad("artifacts", loadBackupArtifacts),
    [loadBackupArtifacts, trackBackupSourceLoad],
  );
  const trackedLoadRestorePlans = useCallback(
    () => trackBackupSourceLoad("restorePlans", loadRestorePlans),
    [loadRestorePlans, trackBackupSourceLoad],
  );
  const trackedLoadMigrationLinks = useCallback(
    () => trackBackupSourceLoad("migrationLinks", loadMigrationLinks),
    [loadMigrationLinks, trackBackupSourceLoad],
  );
  const trackedLoadBackupRequestArtifactProjections = useCallback(() => {
    let sharedLoad: Promise<void> | null = null;
    const load = () => {
      sharedLoad ??= loadBackupRequestArtifactProjections();
      return sharedLoad;
    };
    return Promise.all([
      trackBackupSourceLoad("requests", load),
      trackBackupSourceLoad("artifacts", load),
    ]).then(() => undefined);
  }, [loadBackupRequestArtifactProjections, trackBackupSourceLoad]);

  const createBackupRequest = useCallback(
    async (request: CreateBackupRequest) => {
      const response = await apiPost<BackupRequestRecord>(
        "/api/v1/backups",
        apiToken,
        request,
      );
      if (apiTokenRef.current === apiToken) {
        storeBackupRequest(response);
      }
      await onAuditChanged();
      return response;
    },
    [apiToken, onAuditChanged, storeBackupRequest],
  );

  const createBackupPolicy = useCallback(
    async (request: CreateBackupPolicyRequest) => {
      const response = await apiPost<BackupPolicyRecord>(
        "/api/v1/backup-policies",
        apiToken,
        request,
      );
      if (apiTokenRef.current === apiToken) {
        storeBackupPolicy(response);
      }
      await onAuditChanged();
      return response;
    },
    [apiToken, onAuditChanged, storeBackupPolicy],
  );

  const updateBackupPolicy = useCallback(
    async (scheduleId: string, request: UpdateBackupPolicyRequest) => {
      const response = await apiPut<BackupPolicyRecord>(
        `/api/v1/backup-policies/${encodeURIComponent(scheduleId)}`,
        apiToken,
        request,
      );
      if (apiTokenRef.current === apiToken) {
        storeBackupPolicy(response);
      }
      await onAuditChanged();
      return response;
    },
    [apiToken, onAuditChanged, storeBackupPolicy],
  );

  const pruneBackupPolicies = useCallback(
    async (request: BackupPolicyPruneRequest) => {
      const response = await apiPost<BackupPolicyPruneResponse>(
        "/api/v1/backup-policies/prune",
        apiToken,
        request,
      );
      const refreshPrunedRows =
        !response.dry_run &&
        response.policies.some((policy) => policy.pruned_rows > 0);
      await Promise.all([
        refreshPrunedRows
          ? loadBackupRequestArtifactProjections()
          : Promise.resolve(),
        onAuditChanged(),
      ]);
      return response;
    },
    [apiToken, loadBackupRequestArtifactProjections, onAuditChanged],
  );

  const createRestorePlan = useCallback(
    async (request: CreateRestorePlanRequest) => {
      const response = await apiPost<RestorePlanRecord>(
        "/api/v1/restore-plans",
        apiToken,
        request,
      );
      if (apiTokenRef.current === apiToken) {
        storeRestorePlan(response);
      }
      await onAuditChanged();
      return response;
    },
    [apiToken, onAuditChanged, storeRestorePlan],
  );

  const createMigrationLink = useCallback(
    async (request: CreateMigrationLinkRequest) => {
      const response = await apiPost<MigrationLinkRecord>(
        "/api/v1/migration-links",
        apiToken,
        request,
      );
      if (apiTokenRef.current === apiToken) {
        storeMigrationLink(response);
      }
      await onAuditChanged();
      return response;
    },
    [apiToken, onAuditChanged, storeMigrationLink],
  );

  const createMigrationRun = useCallback(
    async (request: CreateMigrationRunRequest) => {
      const response = await apiPost<CreateMigrationRunResponse>(
        "/api/v1/migration-runs",
        apiToken,
        request,
      );
      if (apiTokenRef.current === apiToken) {
        storeMigrationLink(response.migration_link);
      }
      await onAuditChanged();
      return response;
    },
    [apiToken, onAuditChanged, storeMigrationLink],
  );

  const uploadBackupArtifact = useCallback(
    async (backupRequestId: string, request: UploadBackupArtifactRequest) => {
      const response = await apiPost<BackupArtifactRecord>(
        `/api/v1/backups/${backupRequestId}/artifact`,
        apiToken,
        request,
      );
      if (apiTokenRef.current === apiToken) {
        storeBackupArtifact(response);
      }
      await Promise.all([loadBackupRequests(), onAuditChanged()]);
      return response;
    },
    [apiToken, loadBackupRequests, onAuditChanged, storeBackupArtifact],
  );

  const uploadBackupArtifactChunked = useCallback(
    async (
      backupRequestId: string,
      objectKey: string,
      artifactFile: File,
      confirmed: boolean,
      chunkSizeBytes = BACKUP_ARTIFACT_UPLOAD_CHUNK_BYTES,
    ) => {
      if (!confirmed) {
        throw new Error("Chunked artifact upload requires confirmation");
      }
      if (artifactFile.size <= 0) {
        throw new Error("Artifact file must not be empty");
      }
      const expectedSha256Hex = await sha256FileHex(artifactFile);
      let session: BackupArtifactUploadSessionRecord | null = null;
      try {
        session = await apiPost<BackupArtifactUploadSessionRecord>(
          `/api/v1/backups/${backupRequestId}/artifact-upload-sessions`,
          apiToken,
          {
            object_key: objectKey,
            expected_sha256_hex: expectedSha256Hex,
            expected_size_bytes: artifactFile.size,
            confirmed,
          },
        );
        const effectiveChunkSize = Math.max(
          1,
          Math.min(chunkSizeBytes, session.max_chunk_bytes),
        );
        let offset = session.next_offset_bytes;
        while (offset < artifactFile.size) {
          const end = Math.min(offset + effectiveChunkSize, artifactFile.size);
          const chunk = await readFileSlice(artifactFile, offset, end);
          const view = await apiPost<BackupArtifactUploadSessionRecord>(
            `/api/v1/backups/${backupRequestId}/artifact-upload-sessions/${session.upload_id}/chunks`,
            apiToken,
            {
              offset_bytes: offset,
              data_base64: bytesToBase64(chunk),
            },
          );
          offset = view.next_offset_bytes;
        }
        const response = await apiPost<BackupArtifactRecord>(
          `/api/v1/backups/${backupRequestId}/artifact-upload-sessions/${session.upload_id}/commit`,
          apiToken,
          { confirmed },
        );
        if (apiTokenRef.current === apiToken) {
          storeBackupArtifact(response);
        }
        await Promise.all([loadBackupRequests(), onAuditChanged()]);
        return response;
      } catch (error) {
        let abortError: unknown = null;
        if (session) {
          try {
            await apiPost<BackupArtifactUploadSessionRecord>(
              `/api/v1/backups/${backupRequestId}/artifact-upload-sessions/${session.upload_id}/abort`,
              apiToken,
              { confirmed: true },
            );
          } catch (cleanupError) {
            abortError = cleanupError;
          }
        }
        if (abortError) {
          const uploadMessage =
            error instanceof Error ? error.message : "Artifact upload failed";
          const abortMessage =
            abortError instanceof Error
              ? abortError.message
              : "upload-session cleanup failed";
          throw new Error(
            `${uploadMessage}; upload-session cleanup also failed: ${abortMessage}`,
          );
        }
        throw error;
      }
    },
    [apiToken, loadBackupRequests, onAuditChanged, storeBackupArtifact],
  );

  const handoffBackupArtifact = useCallback(
    async (backupRequestId: string, request: BackupArtifactHandoffRequest) => {
      const response = await apiPost<BackupArtifactHandoffRecord>(
        `/api/v1/backups/${backupRequestId}/artifact-handoff`,
        apiToken,
        request,
      );
      if (apiTokenRef.current === apiToken) {
        storeBackupArtifact(response.artifact);
      }
      await Promise.all([loadBackupRequests(), onAuditChanged()]);
      return response;
    },
    [apiToken, loadBackupRequests, onAuditChanged, storeBackupArtifact],
  );

  const downloadBackupArtifact = useCallback(
    async (backupRequestId: string) => {
      try {
        return await apiGetBlob(
          `/api/v1/backups/${backupRequestId}/artifact`,
          apiToken,
        );
      } catch (error) {
        if (apiTokenRef.current === apiToken && isApiUnauthorized(error)) {
          onUnauthorized();
        }
        throw error;
      }
    },
    [apiToken, onUnauthorized],
  );

  const clearBackups = useCallback(() => {
    apiTokenRef.current = "";
    backupsLoadOperationGeneration.current += 1;
    const generations = backupProjectionGenerationsRef.current;
    generations.artifacts += 1;
    generations.migrationLinks += 1;
    generations.policies += 1;
    generations.requests += 1;
    generations.restorePlans += 1;
    backupLoadConsumersRef.current.artifacts.discardPending([]);
    backupLoadConsumersRef.current.migrationLinks.discardPending([]);
    backupLoadConsumersRef.current.policies.discardPending([]);
    backupLoadConsumersRef.current.requests.discardPending([]);
    backupLoadConsumersRef.current.restorePlans.discardPending([]);
    homeBackupsHydrationRef.current = null;
    backupProjectionFailuresRef.current = emptyBackupProjectionFailures();
    backupSourceLoadTokens.current.clear();
    backupRequestsEvidenceAvailableRef.current = false;
    backupArtifactsEvidenceAvailableRef.current = false;
    backupsRef.current = [];
    backupPoliciesRef.current = [];
    backupArtifactsRef.current = [];
    restorePlansRef.current = [];
    migrationLinksRef.current = [];
    setBackups([]);
    setBackupsTruncated(false);
    setBackupPolicies([]);
    setBackupPoliciesTruncated(false);
    setBackupArtifacts([]);
    setBackupArtifactsTruncated(false);
    setRestorePlans([]);
    setMigrationLinks([]);
    setBackupsError(null);
    setBackupsLoading(false);
    setBackupSourceLoadingVersion((version) => version + 1);
    setBackupsEvidenceAvailable(false);
  }, []);

  return {
    backups,
    beginHomeBackupsHydration,
    backupsTruncated,
    backupPolicies,
    backupPoliciesTruncated,
    backupArtifacts,
    backupArtifactsTruncated,
    restorePlans,
    migrationLinks,
    backupsError,
    backupsEvidenceAvailable,
    backupsLoading,
    backupSourcesError,
    backupSourcesLoading,
    hydrateHomeBackups,
    createBackupRequest,
    createBackupPolicy,
    updateBackupPolicy,
    createMigrationLink,
    createMigrationRun,
    createRestorePlan,
    clearBackups,
    downloadBackupArtifact,
    handoffBackupArtifact,
    pruneBackupPolicies,
    uploadBackupArtifact,
    uploadBackupArtifactChunked,
    loadBackupArtifacts: trackedLoadBackupArtifacts,
    loadBackupPolicies: trackedLoadBackupPolicies,
    loadBackupRequests: trackedLoadBackupRequests,
    loadBackupRequestArtifactProjections:
      trackedLoadBackupRequestArtifactProjections,
    loadMigrationLinks: trackedLoadMigrationLinks,
    loadRestorePlans: trackedLoadRestorePlans,
    loadBackups,
  };
}

function upsertBoundedRecord<T>(
  records: T[],
  record: T,
  key: (record: T) => string,
  limit: number,
): T[] {
  const recordKey = key(record);
  const currentIndex = records.findIndex(
    (candidate) => key(candidate) === recordKey,
  );
  if (currentIndex >= 0) {
    return records.map((candidate, index) =>
      index === currentIndex ? record : candidate,
    );
  }
  return [record, ...records].slice(0, limit);
}
