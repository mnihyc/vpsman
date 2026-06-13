DO $$
DECLARE
    invalid RECORD;
BEGIN
    SELECT status AS value, count(*) AS count
    INTO invalid
    FROM backup_requests
    WHERE status NOT IN ('requested_metadata_only', 'artifact_metadata_recorded')
    GROUP BY status
    LIMIT 1;
    IF FOUND THEN
        RAISE EXCEPTION 'invalid_state:backup_requests.status:value=% count=%', invalid.value, invalid.count;
    END IF;

    SELECT status AS value, count(*) AS count
    INTO invalid
    FROM restore_plans
    WHERE status NOT IN ('planned_metadata_only')
    GROUP BY status
    LIMIT 1;
    IF FOUND THEN
        RAISE EXCEPTION 'invalid_state:restore_plans.status:value=% count=%', invalid.value, invalid.count;
    END IF;

    SELECT status AS value, count(*) AS count
    INTO invalid
    FROM migration_links
    WHERE status NOT IN ('linked_metadata_only')
    GROUP BY status
    LIMIT 1;
    IF FOUND THEN
        RAISE EXCEPTION 'invalid_state:migration_links.status:value=% count=%', invalid.value, invalid.count;
    END IF;

    SELECT status AS value, count(*) AS count
    INTO invalid
    FROM tunnel_plans
    WHERE status NOT IN ('planned', 'applied', 'partially_applied', 'rolled_back', 'partially_rolled_back')
    GROUP BY status
    LIMIT 1;
    IF FOUND THEN
        RAISE EXCEPTION 'invalid_state:tunnel_plans.status:value=% count=%', invalid.value, invalid.count;
    END IF;

    SELECT left_status AS value, count(*) AS count
    INTO invalid
    FROM tunnel_plans
    WHERE left_status NOT IN ('planned', 'applied', 'rolled_back')
    GROUP BY left_status
    LIMIT 1;
    IF FOUND THEN
        RAISE EXCEPTION 'invalid_state:tunnel_plans.left_status:value=% count=%', invalid.value, invalid.count;
    END IF;

    SELECT right_status AS value, count(*) AS count
    INTO invalid
    FROM tunnel_plans
    WHERE right_status NOT IN ('planned', 'applied', 'rolled_back')
    GROUP BY right_status
    LIMIT 1;
    IF FOUND THEN
        RAISE EXCEPTION 'invalid_state:tunnel_plans.right_status:value=% count=%', invalid.value, invalid.count;
    END IF;

    SELECT status AS value, count(*) AS count
    INTO invalid
    FROM agent_update_releases
    WHERE status NOT IN ('published_metadata_only', 'artifact_hosted')
    GROUP BY status
    LIMIT 1;
    IF FOUND THEN
        RAISE EXCEPTION 'invalid_state:agent_update_releases.status:value=% count=%', invalid.value, invalid.count;
    END IF;

    SELECT direction AS value, count(*) AS count
    INTO invalid
    FROM file_transfer_sessions
    WHERE direction NOT IN ('upload', 'download')
    GROUP BY direction
    LIMIT 1;
    IF FOUND THEN
        RAISE EXCEPTION 'invalid_state:file_transfer_sessions.direction:value=% count=%', invalid.value, invalid.count;
    END IF;

    SELECT status AS value, count(*) AS count
    INTO invalid
    FROM file_transfer_sessions
    WHERE status NOT IN ('started', 'transferring', 'completed', 'aborted', 'unknown')
    GROUP BY status
    LIMIT 1;
    IF FOUND THEN
        RAISE EXCEPTION 'invalid_state:file_transfer_sessions.status:value=% count=%', invalid.value, invalid.count;
    END IF;

    SELECT last_event AS value, count(*) AS count
    INTO invalid
    FROM file_transfer_sessions
    WHERE last_event NOT IN (
        'file_transfer_start',
        'file_transfer_chunk_ack',
        'file_transfer_commit',
        'file_transfer_abort',
        'file_transfer_download_start',
        'file_transfer_download_chunk'
    )
    GROUP BY last_event
    LIMIT 1;
    IF FOUND THEN
        RAISE EXCEPTION 'invalid_state:file_transfer_sessions.last_event:value=% count=%', invalid.value, invalid.count;
    END IF;
END
$$;

ALTER TABLE backup_requests
    ADD CONSTRAINT backup_requests_status_check
    CHECK (status IN ('requested_metadata_only', 'artifact_metadata_recorded'));

ALTER TABLE restore_plans
    ADD CONSTRAINT restore_plans_status_check
    CHECK (status IN ('planned_metadata_only'));

ALTER TABLE migration_links
    ADD CONSTRAINT migration_links_status_check
    CHECK (status IN ('linked_metadata_only'));

ALTER TABLE tunnel_plans
    ADD CONSTRAINT tunnel_plans_status_check
    CHECK (status IN ('planned', 'applied', 'partially_applied', 'rolled_back', 'partially_rolled_back')),
    ADD CONSTRAINT tunnel_plans_left_status_check
    CHECK (left_status IN ('planned', 'applied', 'rolled_back')),
    ADD CONSTRAINT tunnel_plans_right_status_check
    CHECK (right_status IN ('planned', 'applied', 'rolled_back'));

ALTER TABLE agent_update_releases
    ADD CONSTRAINT agent_update_releases_status_check
    CHECK (status IN ('published_metadata_only', 'artifact_hosted'));

ALTER TABLE file_transfer_sessions
    ADD CONSTRAINT file_transfer_sessions_direction_check
    CHECK (direction IN ('upload', 'download')),
    ADD CONSTRAINT file_transfer_sessions_status_check
    CHECK (status IN ('started', 'transferring', 'completed', 'aborted', 'unknown')),
    ADD CONSTRAINT file_transfer_sessions_last_event_check
    CHECK (
        last_event IN (
            'file_transfer_start',
            'file_transfer_chunk_ack',
            'file_transfer_commit',
            'file_transfer_abort',
            'file_transfer_download_start',
            'file_transfer_download_chunk'
        )
    );
