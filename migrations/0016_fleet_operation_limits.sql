ALTER TABLE public.schedules
    DROP CONSTRAINT schedules_target_client_ids_limit,
    ADD CONSTRAINT schedules_target_client_ids_limit
        CHECK (cardinality(target_client_ids) BETWEEN 0 AND 1000);
