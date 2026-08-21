# Five-year retained-telemetry pressure proof

The qualification workload exercises a 120-client vnStat import and reimport,
the retention worker, and five concurrent browser sessions over five years of
retained telemetry.

It checks exact raw, rollup, hourly, revision, epoch, job-lineage, live-boundary,
and atomic-failure data; conservation across a zero-write rotation; private and
public history coverage; request bounds; browser health; and database activity
under overlap. Temporary files, rollbacks, deadlocks, idle transactions, and
unexpected database errors are hard failures.

Import timings are workload measurements, not universal API latency promises.
The importer uses a dense locked-key proof with an exact savepoint fallback, and
the retained semantic snapshot uses indexed latest-row probes with explicit
stream-shape and revision checks. These bounds preserve the data contract while
keeping the qualification workload deterministic.
