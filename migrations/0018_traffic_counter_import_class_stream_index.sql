-- no-transaction
-- Import replacement needs the nearest live row and an early-stop count of
-- importer-owned rows in one exact counter stream.  Keeping the row class in
-- the ordered key lets both probes stop at their LIMIT without walking the
-- global observed-at index or sorting a whole stream.  One expression index
-- costs one entry per sample and one concurrent table build; two complementary
-- partial indexes would have the same entry/write count but require two builds.
-- PostgreSQL can retain an invalid index when a concurrent build is canceled.
-- IF NOT EXISTS lets SQLx ledger a retry; the API/worker startup contract then
-- validates the exact catalog shape and repairs only this exact invalid index
-- outside a transaction before either service accepts work.
CREATE INDEX CONCURRENTLY IF NOT EXISTS traffic_counter_samples_import_class_stream_idx
    ON public.traffic_counter_samples (
        client_id,
        source_kind,
        interface,
        (sample_source LIKE 'vnstat_import:%'),
        observed_at
    );
