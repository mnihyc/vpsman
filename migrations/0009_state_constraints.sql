ALTER TABLE command_templates
    ADD COLUMN IF NOT EXISTS display_group TEXT;

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

    SELECT last_command_type AS value, count(*) AS count
    INTO invalid
    FROM file_transfer_sessions
    WHERE last_command_type NOT IN (
        'file_transfer_start',
        'file_transfer_chunk',
        'file_transfer_commit',
        'file_transfer_abort',
        'file_transfer_download_start',
        'file_transfer_download_chunk'
    )
    GROUP BY last_command_type
    LIMIT 1;
    IF FOUND THEN
        RAISE EXCEPTION 'invalid_state:file_transfer_sessions.last_command_type:value=% count=%', invalid.value, invalid.count;
    END IF;

    SELECT state AS value, count(*) AS count
    INTO invalid
    FROM terminal_sessions
    WHERE state NOT IN ('open', 'closed', 'missing', 'rejected', 'exited', 'unknown')
    GROUP BY state
    LIMIT 1;
    IF FOUND THEN
        RAISE EXCEPTION 'invalid_state:terminal_sessions.state:value=% count=%', invalid.value, invalid.count;
    END IF;

    SELECT last_status AS value, count(*) AS count
    INTO invalid
    FROM terminal_sessions
    WHERE last_status NOT IN (
        'opened',
        'attached',
        'rejected',
        'accepted',
        'duplicate_ignored',
        'polled',
        'resized',
        'closed',
        'missing',
        'streaming',
        'exited',
        'idle_timeout',
        'unknown'
    )
    GROUP BY last_status
    LIMIT 1;
    IF FOUND THEN
        RAISE EXCEPTION 'invalid_state:terminal_sessions.last_status:value=% count=%', invalid.value, invalid.count;
    END IF;

    SELECT last_event AS value, count(*) AS count
    INTO invalid
    FROM terminal_sessions
    WHERE last_event NOT IN (
        'terminal_open',
        'terminal_input',
        'terminal_poll',
        'terminal_resize',
        'terminal_close',
        'terminal_stream'
    )
    GROUP BY last_event
    LIMIT 1;
    IF FOUND THEN
        RAISE EXCEPTION 'invalid_state:terminal_sessions.last_event:value=% count=%', invalid.value, invalid.count;
    END IF;

    SELECT last_command_type AS value, count(*) AS count
    INTO invalid
    FROM terminal_sessions
    WHERE last_command_type NOT IN (
        'terminal_open',
        'terminal_input',
        'terminal_poll',
        'terminal_resize',
        'terminal_close'
    )
    GROUP BY last_command_type
    LIMIT 1;
    IF FOUND THEN
        RAISE EXCEPTION 'invalid_state:terminal_sessions.last_command_type:value=% count=%', invalid.value, invalid.count;
    END IF;

    SELECT command_type AS value, count(*) AS count
    INTO invalid
    FROM command_templates
    WHERE command_type NOT IN (
        'shell_argv',
        'shell_pty',
        'shell_script',
        'terminal_open',
        'terminal_input',
        'terminal_poll',
        'terminal_resize',
        'terminal_close',
        'config_read',
        'hot_config',
        'data_source_config_patch',
        'agent_update',
        'agent_update_activate',
        'agent_update_rollback',
        'agent_update_check',
        'file_pull',
        'file_push',
        'file_push_chunked',
        'file_transfer_start',
        'file_transfer_chunk',
        'file_transfer_commit',
        'file_transfer_abort',
        'file_transfer_download_start',
        'file_transfer_download_chunk',
        'file_stat',
        'file_list_dir',
        'file_read_text',
        'file_mkdir',
        'file_write_text',
        'file_rename',
        'file_delete',
        'file_chmod',
        'file_chown',
        'file_copy',
        'file_download',
        'file_archive_tar',
        'user_sessions',
        'process_list',
        'process_start',
        'process_stop',
        'process_restart',
        'process_status',
        'process_logs',
        'backup',
        'restore',
        'restore_rollback',
        'network_apply',
        'network_ospf_cost_update',
        'network_rollback',
        'network_status',
        'network_interfaces',
        'network_probe',
        'network_speed_test'
    )
    GROUP BY command_type
    LIMIT 1;
    IF FOUND THEN
        RAISE EXCEPTION 'invalid_state:command_templates.command_type:value=% count=%', invalid.value, invalid.count;
    END IF;

    SELECT display_group AS value, count(*) AS count
    INTO invalid
    FROM command_templates
    WHERE display_group IS NOT NULL
      AND (length(display_group) = 0 OR length(display_group) > 64)
    GROUP BY display_group
    LIMIT 1;
    IF FOUND THEN
        RAISE EXCEPTION 'invalid_state:command_templates.display_group:value=% count=%', invalid.value, invalid.count;
    END IF;

    SELECT job_type AS value, count(*) AS count
    INTO invalid
    FROM server_jobs
    WHERE job_type NOT IN ('artifact_cleanup')
    GROUP BY job_type
    LIMIT 1;
    IF FOUND THEN
        RAISE EXCEPTION 'invalid_state:server_jobs.job_type:value=% count=%', invalid.value, invalid.count;
    END IF;

    SELECT status AS value, count(*) AS count
    INTO invalid
    FROM server_jobs
    WHERE status NOT IN ('queued', 'running', 'completed', 'failed', 'canceled')
    GROUP BY status
    LIMIT 1;
    IF FOUND THEN
        RAISE EXCEPTION 'invalid_state:server_jobs.status:value=% count=%', invalid.value, invalid.count;
    END IF;

    SELECT status AS value, count(*) AS count
    INTO invalid
    FROM fleet_alert_notification_deliveries
    WHERE status NOT IN ('queued', 'failed', 'delivered', 'matched_dry_run')
    GROUP BY status
    LIMIT 1;
    IF FOUND THEN
        RAISE EXCEPTION 'invalid_state:fleet_alert_notification_deliveries.status:value=% count=%', invalid.value, invalid.count;
    END IF;

    SELECT status AS value, count(*) AS count
    INTO invalid
    FROM webhook_rule_deliveries
    WHERE status NOT IN ('queued', 'failed', 'permanently_failed', 'delivered', 'matched_dry_run')
    GROUP BY status
    LIMIT 1;
    IF FOUND THEN
        RAISE EXCEPTION 'invalid_state:webhook_rule_deliveries.status:value=% count=%', invalid.value, invalid.count;
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
    ),
    ADD CONSTRAINT file_transfer_sessions_last_command_type_check
    CHECK (
        last_command_type IN (
            'file_transfer_start',
            'file_transfer_chunk',
            'file_transfer_commit',
            'file_transfer_abort',
            'file_transfer_download_start',
            'file_transfer_download_chunk'
        )
    );

ALTER TABLE terminal_sessions
    ADD CONSTRAINT terminal_sessions_state_check
    CHECK (state IN ('open', 'closed', 'missing', 'rejected', 'exited', 'unknown')),
    ADD CONSTRAINT terminal_sessions_last_status_check
    CHECK (
        last_status IN (
            'opened',
            'attached',
            'rejected',
            'accepted',
            'duplicate_ignored',
            'polled',
            'resized',
            'closed',
            'missing',
            'streaming',
            'exited',
            'idle_timeout',
            'unknown'
        )
    ),
    ADD CONSTRAINT terminal_sessions_last_event_check
    CHECK (
        last_event IN (
            'terminal_open',
            'terminal_input',
            'terminal_poll',
            'terminal_resize',
            'terminal_close',
            'terminal_stream'
        )
    ),
    ADD CONSTRAINT terminal_sessions_last_command_type_check
    CHECK (
        last_command_type IN (
            'terminal_open',
            'terminal_input',
            'terminal_poll',
            'terminal_resize',
            'terminal_close'
        )
    );

ALTER TABLE command_templates
    ADD CONSTRAINT command_templates_display_group_check
    CHECK (display_group IS NULL OR length(display_group) BETWEEN 1 AND 64),
    ADD CONSTRAINT command_templates_command_type_check
    CHECK (
        command_type IN (
            'shell_argv',
            'shell_pty',
            'shell_script',
            'terminal_open',
            'terminal_input',
            'terminal_poll',
            'terminal_resize',
            'terminal_close',
            'config_read',
            'hot_config',
            'data_source_config_patch',
            'agent_update',
            'agent_update_activate',
            'agent_update_rollback',
            'agent_update_check',
            'file_pull',
            'file_push',
            'file_push_chunked',
            'file_transfer_start',
            'file_transfer_chunk',
            'file_transfer_commit',
            'file_transfer_abort',
            'file_transfer_download_start',
            'file_transfer_download_chunk',
            'file_stat',
            'file_list_dir',
            'file_read_text',
            'file_mkdir',
            'file_write_text',
            'file_rename',
            'file_delete',
            'file_chmod',
            'file_chown',
            'file_copy',
            'file_download',
            'file_archive_tar',
            'user_sessions',
            'process_list',
            'process_start',
            'process_stop',
            'process_restart',
            'process_status',
            'process_logs',
            'backup',
            'restore',
            'restore_rollback',
            'network_apply',
            'network_ospf_cost_update',
            'network_rollback',
            'network_status',
            'network_interfaces',
            'network_probe',
            'network_speed_test'
        )
    );

ALTER TABLE server_jobs
    ADD CONSTRAINT server_jobs_type_check_0009
    CHECK (job_type IN ('artifact_cleanup')),
    ADD CONSTRAINT server_jobs_status_check_0009
    CHECK (status IN ('queued', 'running', 'completed', 'failed', 'canceled'));

ALTER TABLE fleet_alert_notification_deliveries
    ADD CONSTRAINT fleet_alert_notification_deliveries_status_check_0009
    CHECK (status IN ('queued', 'failed', 'delivered', 'matched_dry_run'));

ALTER TABLE webhook_rule_deliveries
    ADD CONSTRAINT webhook_rule_deliveries_status_check_0009
    CHECK (status IN ('queued', 'failed', 'permanently_failed', 'delivered', 'matched_dry_run'));
