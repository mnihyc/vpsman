import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { apiGet, apiPost, ApiResponseError, LatestReadConsumer } from "../api";
import type { AuthSession } from "../authSession";
import type { ConsoleDataGridRemote, ConsoleDataGridRemoteRequest } from "../components/ConsoleDataGrid";
import type { SearchCompletion } from "../components/SearchExpressionInput";
import {
  JOB_SEARCH_HELP, jobSearchCompletion, lookupKey,
  type JobSearchField, type JobSearchPage, type JobSearchValue,
} from "../jobHistorySearch";
import { parseSearchExpression } from "../searchExpression";
import type { JobDetailsInvalidationSignal, JobHistoryRecord } from "../types";

// Wait for a short typing pause, not one SQL query per keystroke. Paging,
// Refresh and live invalidations use the existing immediate read consumer.
const SEARCH_TYPING_DELAY_MS = 200;

export function useJobHistorySearch(
  apiToken: AuthSession | null,
  enabled: boolean,
  recentJobs: JobHistoryRecord[],
  invalidation: JobDetailsInvalidationSignal | null,
) {
  const [request, setRequest] = useState<ConsoleDataGridRemoteRequest | null>(null);
  const requested = useRef<ConsoleDataGridRemoteRequest | null>(null);
  const [pageIndex, setPageIndex] = useState(0);
  const [rows, setRows] = useState<JobHistoryRecord[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [queryError, setQueryError] = useState<string | null>(null);
  const [nextCursor, setNextCursor] = useState<string | null>(null);
  const [total, setTotal] = useState(0);
  const [refreshVersion, setRefreshVersion] = useState(0);
  const [fields, setFields] = useState<JobSearchField[]>([]);
  const [lookup, setLookup] = useState<SearchCompletion["lookup"]>();
  const [hintValues, setHintValues] = useState(new Map<string, JobSearchValue[]>());
  const [hintError, setHintError] = useState<string | null>(null);
  const cursors = useRef<Array<string | null>>([null]);
  const generation = useRef(0);
  const active = useRef<AbortController | null>(null);
  const reads = useRef(new LatestReadConsumer<void>());
  const previousQuery = useRef<string | null>(null);
  const selection = JSON.stringify([apiToken?.epoch, enabled, request, pageIndex]);
  const desiredSelection = useRef(selection);
  desiredSelection.current = selection;

  const onRequest = useCallback((next: ConsoleDataGridRemoteRequest) => {
    if (JSON.stringify(requested.current) === JSON.stringify(next)) return;
    requested.current = next;
    generation.current += 1;
    active.current?.abort();
    cursors.current = [null];
    setPageIndex(0);
    setNextCursor(null);
    setTotal(0);
    setRequest(next);
  }, []);
  const onPageChange = useCallback((index: number) => {
    if (index < 0 || index >= cursors.current.length) return;
    generation.current += 1;
    active.current?.abort();
    setPageIndex(index);
  }, []);
  const refresh = useCallback(() => setRefreshVersion((value) => value + 1), []);

  useEffect(() => {
    generation.current += 1;
    active.current?.abort();
    reads.current.discardPending();
    cursors.current = [null];
    setPageIndex(0);
    setRows([]);
    setTotal(0);
    setNextCursor(null);
    setError(null);
    setQueryError(null);
    setFields([]);
    setHintValues(new Map());
    return () => {
      generation.current += 1;
      active.current?.abort();
      reads.current.discardPending();
    };
  }, [apiToken]);

  useEffect(() => {
    const revision = ++generation.current;
    if (!enabled || !apiToken || !request) {
      active.current?.abort();
      setLoading(false);
      return;
    }
    const parsed = parseSearchExpression(request.query);
    setError(parsed.error);
    setQueryError(parsed.error);
    if (parsed.error) {
      setLoading(false);
      return;
    }
    setLoading(true);
    const typing = previousQuery.current !== request.query;
    previousQuery.current = request.query;
    const timer = window.setTimeout(() => {
      void reads.current.enqueue(async () => {
        if (generation.current !== revision || desiredSelection.current !== selection) return;
        const controller = new AbortController();
        active.current = controller;
        try {
          const page = await apiPost<JobSearchPage>("/api/v1/jobs/search", apiToken, {
            q: request.query, sort: request.sorting, limit: request.pageSize,
            cursor: cursors.current[pageIndex] ?? null,
          }, controller.signal);
          if (generation.current !== revision || desiredSelection.current !== selection || controller.signal.aborted) return;
          if (page.rows.length === 0 && pageIndex > 0) {
            // Live results can retire a whole page. Move to the nearest known
            // earlier cursor, retaining the query and selected job details.
            setTotal(page.total);
            onPageChange(Math.min(pageIndex - 1, Math.max(0, Math.ceil(page.total / request.pageSize) - 1)));
            return;
          }
          setRows(page.rows);
          setTotal(page.total);
          setNextCursor(page.next_cursor);
          cursors.current = cursors.current.slice(0, pageIndex + 1);
          if (page.next_cursor) cursors.current[pageIndex + 1] = page.next_cursor;
          setError(null);
          setQueryError(null);
        } catch (cause) {
          if (generation.current !== revision || desiredSelection.current !== selection || controller.signal.aborted) return;
          const message = cause instanceof ApiResponseError
            ? cause.detail ?? cause.message
            : cause instanceof Error ? cause.message : "Job history could not be searched.";
          setError(message);
          setQueryError(cause instanceof ApiResponseError && cause.code === "invalid_job_search" ? message : null);
        } finally {
          if (generation.current === revision && desiredSelection.current === selection) setLoading(false);
          if (active.current === controller) active.current = null;
        }
      });
    }, typing ? SEARCH_TYPING_DELAY_MS : 0);
    return () => window.clearTimeout(timer);
  }, [selection, apiToken, enabled, request, pageIndex, recentJobs, invalidation?.generation, refreshVersion, onPageChange]);

  useEffect(() => {
    if (!enabled || !apiToken || fields.length) return;
    let current = true;
    void apiGet<JobSearchField[]>("/api/v1/jobs/search/fields", apiToken).then((values) => {
      if (current) { setFields(values); setHintError(null); }
    }).catch(() => { if (current) setHintError("Search hints are unavailable; Refresh to retry."); });
    return () => { current = false; };
  }, [apiToken, enabled, refreshVersion, fields.length]);

  const onCompletionLookup = useCallback((next: SearchCompletion["lookup"]) => {
    setLookup((current) => JSON.stringify(current) === JSON.stringify(next) ? current : next);
  }, []);
  useEffect(() => {
    if (!enabled || !apiToken || !lookup || hintValues.has(lookupKey(lookup))) return;
    let current = true;
    const timer = window.setTimeout(() => {
      const parameters = new URLSearchParams(lookup);
      void apiGet<JobSearchValue[]>("/api/v1/jobs/search/values?" + parameters, apiToken).then((values) => {
        if (current) setHintValues(new Map([[lookupKey(lookup), values]]));
      }).catch(() => { /* Suggestions remain optional; authoritative search still runs. */ });
    }, SEARCH_TYPING_DELAY_MS);
    return () => { current = false; window.clearTimeout(timer); };
  }, [apiToken, enabled, lookup, hintValues]);

  const completionProvider = useCallback((value: string, caret: number) =>
    jobSearchCompletion(value, caret, fields, hintValues), [fields, hintValues]);
  const remote = useMemo<ConsoleDataGridRemote>(() => ({
    onRequest, onPageChange, pageIndex, hasNextPage: Boolean(nextCursor), total, loading,
    error, queryError, help: hintError ?? JOB_SEARCH_HELP, completionProvider, onCompletionLookup,
  }), [onRequest, onPageChange, pageIndex, nextCursor, total, loading, error, queryError, hintError, completionProvider, onCompletionLookup]);
  return { rows, remote, refresh, loading };
}
