-- Add explicit policy-alert lifecycle evidence without inventing a current
-- state for alerts created by the pre-0010 schema.

ALTER TABLE policy_alerts
    -- The temporary default is the backfill for rows whose pre-0010 activity
    -- cannot be reconstructed. It is changed to triggered for new rows below.
    ADD COLUMN lifecycle_state TEXT NOT NULL DEFAULT 'unknown',
    ADD COLUMN last_confirmed_at TIMESTAMPTZ,
    ADD COLUMN resolved_at TIMESTAMPTZ,
    ADD COLUMN resolution_reason TEXT;

ALTER TABLE policy_alerts
    ALTER COLUMN lifecycle_state SET DEFAULT 'triggered',
    ADD CONSTRAINT policy_alerts_lifecycle_check CHECK (
        (
            lifecycle_state IN ('triggered', 'persisting')
            AND last_confirmed_at IS NOT NULL
            AND last_confirmed_at >= created_at
            AND resolved_at IS NULL
            AND resolution_reason IS NULL
        ) OR (
            lifecycle_state = 'unknown'
            AND (last_confirmed_at IS NULL OR last_confirmed_at >= created_at)
            AND resolved_at IS NULL
            AND resolution_reason IS NULL
        ) OR (
            lifecycle_state = 'resolved'
            AND last_confirmed_at IS NOT NULL
            AND last_confirmed_at >= created_at
            AND resolved_at IS NOT NULL
            AND resolved_at >= last_confirmed_at
            AND resolution_reason IN (
                'condition_recovered',
                'policy_scope_exited',
                'policy_scope_changed',
                'policy_disabled',
                'policy_changed',
                'policy_deleted'
            )
        )
    );

CREATE INDEX policy_alerts_current_fleet_priority_idx
    ON policy_alerts (
        (CASE
            WHEN lifecycle_state IN ('triggered', 'persisting') THEN 0
            ELSE 1
        END),
        (CASE severity
            WHEN 'critical' THEN 0
            WHEN 'warning' THEN 1
            WHEN 'info' THEN 2
            ELSE 3
        END),
        observed_at DESC,
        id DESC
    )
    WHERE resolved_at IS NULL
      AND last_confirmed_at IS NOT NULL;

CREATE INDEX policy_alerts_current_client_fleet_priority_idx
    ON policy_alerts (
        client_id,
        (CASE
            WHEN lifecycle_state IN ('triggered', 'persisting') THEN 0
            ELSE 1
        END),
        (CASE severity
            WHEN 'critical' THEN 0
            WHEN 'warning' THEN 1
            WHEN 'info' THEN 2
            ELSE 3
        END),
        observed_at DESC,
        id DESC
    )
    WHERE resolved_at IS NULL
      AND last_confirmed_at IS NOT NULL;

-- Existing alerts intentionally enter the explicit lifecycle as unknown,
-- without a last-confirmed or resolution timestamp. New rows default to
-- triggered and must carry confirmation evidence. The pre-0010 schema did not
-- retain enough evidence to reconstruct trustworthy activity or recovery.

-- cpu.load_saturation now evaluates normalized load-per-core rather than raw
-- load. Reset only state derived from that exact identifier so the next policy
-- pass evaluates the corrected meaning as a new rule version. Historical alert
-- evidence remains immutable.
DELETE FROM policy_rule_states AS state
USING policy_rules AS rule
WHERE state.policy_rule_id = rule.id
  AND rule.condition_expression ~
      '(^|[^A-Za-z0-9_.])cpu[.]load_saturation([^A-Za-z0-9_.]|$)';

UPDATE policy_rules
SET rule_version = rule_version + 1,
    updated_at = now()
WHERE condition_expression ~
      '(^|[^A-Za-z0-9_.])cpu[.]load_saturation([^A-Za-z0-9_.]|$)';

-- Replace untouched legacy resource-policy starters with disabled policies
-- expressed in the same persisted policy model operators use. Existing policy
-- intent wins: a deleted legacy starter stays deleted, while an edited,
-- renamed, or enabled starter remains beside the new disabled replacement.

INSERT INTO policy_groups (
    id, name, enabled, selector_expression, notes
)
SELECT
    'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaa4',
    'Predefined CPU utilization',
    FALSE,
    'status:online',
    'Disabled predefined policy. Enable or edit after confirming fleet-specific CPU utilization expectations.'
WHERE EXISTS (
    SELECT 1
    FROM policy_groups
    WHERE id = 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaa1'
)
ON CONFLICT DO NOTHING;

INSERT INTO policy_groups (
    id, name, enabled, selector_expression, notes
)
SELECT
    'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaa5',
    'Predefined memory availability',
    FALSE,
    'status:online',
    'Disabled predefined policy. Enable or edit after confirming fleet-specific memory availability thresholds.'
WHERE EXISTS (
    SELECT 1
    FROM policy_groups
    WHERE id = 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaa2'
)
ON CONFLICT DO NOTHING;

INSERT INTO policy_groups (
    id, name, enabled, selector_expression, notes
) VALUES (
    'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaa6',
    'Predefined disk availability',
    FALSE,
    'status:online',
    'Disabled predefined policy. Enable or edit after confirming fleet-specific disk availability thresholds.'
)
ON CONFLICT DO NOTHING;

INSERT INTO policy_rules (
    id,
    group_id,
    sort_order,
    name,
    enabled,
    traffic_selector,
    condition_expression,
    window_secs,
    severity
)
SELECT
    'bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbb4',
    groups.id,
    0,
    'CPU utilization from 75% to 90%',
    TRUE,
    NULL,
    'cpu.utilization_ratio >= 0.75 && cpu.utilization_ratio < 0.90',
    300,
    'warning'
FROM policy_groups AS groups
WHERE groups.id = 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaa4'
  AND groups.name = 'Predefined CPU utilization'
  AND groups.enabled = FALSE
  AND groups.selector_expression = 'status:online'
  AND groups.notes = 'Disabled predefined policy. Enable or edit after confirming fleet-specific CPU utilization expectations.'
  AND groups.created_by IS NULL
  AND groups.updated_by IS NULL
ON CONFLICT DO NOTHING;

INSERT INTO policy_rules (
    id,
    group_id,
    sort_order,
    name,
    enabled,
    traffic_selector,
    condition_expression,
    window_secs,
    severity
)
SELECT
    'bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbb5',
    groups.id,
    1,
    'CPU utilization at or above 90%',
    TRUE,
    NULL,
    'cpu.utilization_ratio >= 0.90',
    300,
    'critical'
FROM policy_groups AS groups
WHERE groups.id = 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaa4'
  AND groups.name = 'Predefined CPU utilization'
  AND groups.enabled = FALSE
  AND groups.selector_expression = 'status:online'
  AND groups.notes = 'Disabled predefined policy. Enable or edit after confirming fleet-specific CPU utilization expectations.'
  AND groups.created_by IS NULL
  AND groups.updated_by IS NULL
ON CONFLICT DO NOTHING;

INSERT INTO policy_rules (
    id,
    group_id,
    sort_order,
    name,
    enabled,
    traffic_selector,
    condition_expression,
    window_secs,
    severity
)
SELECT
    'bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbb6',
    groups.id,
    0,
    'Available memory from 10% to 20%',
    TRUE,
    NULL,
    'memory.available_ratio <= 0.20 && memory.available_ratio > 0.10',
    300,
    'warning'
FROM policy_groups AS groups
WHERE groups.id = 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaa5'
  AND groups.name = 'Predefined memory availability'
  AND groups.enabled = FALSE
  AND groups.selector_expression = 'status:online'
  AND groups.notes = 'Disabled predefined policy. Enable or edit after confirming fleet-specific memory availability thresholds.'
  AND groups.created_by IS NULL
  AND groups.updated_by IS NULL
ON CONFLICT DO NOTHING;

INSERT INTO policy_rules (
    id,
    group_id,
    sort_order,
    name,
    enabled,
    traffic_selector,
    condition_expression,
    window_secs,
    severity
)
SELECT
    'bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbb7',
    groups.id,
    1,
    'Available memory at or below 10%',
    TRUE,
    NULL,
    'memory.available_ratio <= 0.10',
    300,
    'critical'
FROM policy_groups AS groups
WHERE groups.id = 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaa5'
  AND groups.name = 'Predefined memory availability'
  AND groups.enabled = FALSE
  AND groups.selector_expression = 'status:online'
  AND groups.notes = 'Disabled predefined policy. Enable or edit after confirming fleet-specific memory availability thresholds.'
  AND groups.created_by IS NULL
  AND groups.updated_by IS NULL
ON CONFLICT DO NOTHING;

INSERT INTO policy_rules (
    id,
    group_id,
    sort_order,
    name,
    enabled,
    traffic_selector,
    condition_expression,
    window_secs,
    severity
)
SELECT
    'bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbb8',
    groups.id,
    0,
    'Available disk from 10% to 20%',
    TRUE,
    NULL,
    'disk.available_ratio <= 0.20 && disk.available_ratio > 0.10',
    300,
    'warning'
FROM policy_groups AS groups
WHERE groups.id = 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaa6'
  AND groups.name = 'Predefined disk availability'
  AND groups.enabled = FALSE
  AND groups.selector_expression = 'status:online'
  AND groups.notes = 'Disabled predefined policy. Enable or edit after confirming fleet-specific disk availability thresholds.'
  AND groups.created_by IS NULL
  AND groups.updated_by IS NULL
ON CONFLICT DO NOTHING;

INSERT INTO policy_rules (
    id,
    group_id,
    sort_order,
    name,
    enabled,
    traffic_selector,
    condition_expression,
    window_secs,
    severity
)
SELECT
    'bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbb9',
    groups.id,
    1,
    'Available disk at or below 10%',
    TRUE,
    NULL,
    'disk.available_ratio <= 0.10',
    300,
    'critical'
FROM policy_groups AS groups
WHERE groups.id = 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaa6'
  AND groups.name = 'Predefined disk availability'
  AND groups.enabled = FALSE
  AND groups.selector_expression = 'status:online'
  AND groups.notes = 'Disabled predefined policy. Enable or edit after confirming fleet-specific disk availability thresholds.'
  AND groups.created_by IS NULL
  AND groups.updated_by IS NULL
ON CONFLICT DO NOTHING;

-- Only remove a legacy starter when it is byte-for-byte equivalent to the
-- shipped seed, has never been operator-owned, has no evaluation or alert
-- history, and its complete disabled replacement exists.
DELETE FROM policy_groups AS legacy
WHERE legacy.id = 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaa1'
  AND legacy.name = 'Predefined CPU load warning'
  AND legacy.enabled = FALSE
  AND legacy.selector_expression = 'status:online'
  AND legacy.notes = 'Disabled predefined policy. Enable or edit after confirming fleet-specific CPU expectations.'
  AND legacy.created_by IS NULL
  AND legacy.updated_by IS NULL
  AND legacy.updated_at = legacy.created_at
  AND (
      SELECT count(*)
      FROM policy_rules
      WHERE group_id = legacy.id
  ) = 1
  AND EXISTS (
      SELECT 1
      FROM policy_rules AS rule
      WHERE rule.id = 'bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbb1'
        AND rule.group_id = legacy.id
        AND rule.rule_version = 1
        AND rule.sort_order = 0
        AND rule.name = 'CPU load above 1.5'
        AND rule.enabled = TRUE
        AND rule.traffic_selector IS NULL
        AND rule.condition_expression = 'cpu.load_1 >= 1.5'
        AND rule.window_secs = 300
        AND rule.severity = 'warning'
        AND rule.updated_at = rule.created_at
  )
  AND NOT EXISTS (
      SELECT 1
      FROM policy_rule_states AS state
      JOIN policy_rules AS rule ON rule.id = state.policy_rule_id
      WHERE rule.group_id = legacy.id
  )
  AND NOT EXISTS (
      SELECT 1
      FROM policy_alerts AS alert
      WHERE alert.policy_group_id = legacy.id
         OR alert.policy_rule_id = 'bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbb1'
  )
  AND EXISTS (
      SELECT 1
      FROM policy_groups AS replacement
      WHERE replacement.id = 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaa4'
        AND replacement.name = 'Predefined CPU utilization'
        AND replacement.enabled = FALSE
        AND replacement.selector_expression = 'status:online'
        AND replacement.created_by IS NULL
        AND replacement.updated_by IS NULL
        AND (
            SELECT count(*)
            FROM policy_rules
            WHERE group_id = replacement.id
        ) = 2
        AND EXISTS (
            SELECT 1 FROM policy_rules
            WHERE id = 'bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbb4'
              AND group_id = replacement.id
        )
        AND EXISTS (
            SELECT 1 FROM policy_rules
            WHERE id = 'bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbb5'
              AND group_id = replacement.id
        )
  );

DELETE FROM policy_groups AS legacy
WHERE legacy.id = 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaa2'
  AND legacy.name = 'Predefined memory pressure'
  AND legacy.enabled = FALSE
  AND legacy.selector_expression = 'status:online'
  AND legacy.notes = 'Disabled predefined policy. Enable or edit after confirming memory pressure thresholds.'
  AND legacy.created_by IS NULL
  AND legacy.updated_by IS NULL
  AND legacy.updated_at = legacy.created_at
  AND (
      SELECT count(*)
      FROM policy_rules
      WHERE group_id = legacy.id
  ) = 1
  AND EXISTS (
      SELECT 1
      FROM policy_rules AS rule
      WHERE rule.id = 'bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbb2'
        AND rule.group_id = legacy.id
        AND rule.rule_version = 1
        AND rule.sort_order = 0
        AND rule.name = 'Available memory below 15%'
        AND rule.enabled = TRUE
        AND rule.traffic_selector IS NULL
        AND rule.condition_expression = 'memory.available_ratio <= 0.15'
        AND rule.window_secs = 300
        AND rule.severity = 'warning'
        AND rule.updated_at = rule.created_at
  )
  AND NOT EXISTS (
      SELECT 1
      FROM policy_rule_states AS state
      JOIN policy_rules AS rule ON rule.id = state.policy_rule_id
      WHERE rule.group_id = legacy.id
  )
  AND NOT EXISTS (
      SELECT 1
      FROM policy_alerts AS alert
      WHERE alert.policy_group_id = legacy.id
         OR alert.policy_rule_id = 'bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbb2'
  )
  AND EXISTS (
      SELECT 1
      FROM policy_groups AS replacement
      WHERE replacement.id = 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaa5'
        AND replacement.name = 'Predefined memory availability'
        AND replacement.enabled = FALSE
        AND replacement.selector_expression = 'status:online'
        AND replacement.created_by IS NULL
        AND replacement.updated_by IS NULL
        AND (
            SELECT count(*)
            FROM policy_rules
            WHERE group_id = replacement.id
        ) = 2
        AND EXISTS (
            SELECT 1 FROM policy_rules
            WHERE id = 'bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbb6'
              AND group_id = replacement.id
        )
        AND EXISTS (
            SELECT 1 FROM policy_rules
            WHERE id = 'bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbb7'
              AND group_id = replacement.id
        )
  );
