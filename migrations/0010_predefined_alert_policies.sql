INSERT INTO policy_groups (
    id, name, enabled, selector_expression, notes
) VALUES
    (
        'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaa1',
        'Predefined CPU load warning',
        FALSE,
        'status:online',
        'Disabled predefined policy. Enable or edit after confirming fleet-specific CPU expectations.'
    ),
    (
        'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaa2',
        'Predefined memory pressure',
        FALSE,
        'status:online',
        'Disabled predefined policy. Enable or edit after confirming memory pressure thresholds.'
    ),
    (
        'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaa3',
        'Predefined traffic quota warning',
        FALSE,
        'status:online',
        'Disabled predefined policy. Enable or edit after configuring VPS traffic quota rules.'
    )
ON CONFLICT (name) DO NOTHING;

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
    'bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbb1',
    'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaa1',
    0,
    'CPU load above 1.5',
    TRUE,
    NULL,
    'cpu.load_1 >= 1.5',
    300,
    'warning'
WHERE EXISTS (
    SELECT 1 FROM policy_groups
    WHERE id = 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaa1'
)
ON CONFLICT (id) DO NOTHING;

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
    'bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbb2',
    'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaa2',
    0,
    'Available memory below 15%',
    TRUE,
    NULL,
    'memory.available_ratio <= 0.15',
    300,
    'warning'
WHERE EXISTS (
    SELECT 1 FROM policy_groups
    WHERE id = 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaa2'
)
ON CONFLICT (id) DO NOTHING;

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
    'bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbb3',
    'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaa3',
    0,
    'Traffic cycle above 80%',
    TRUE,
    NULL,
    'traffic.cycle_percent >= 80',
    300,
    'warning'
WHERE EXISTS (
    SELECT 1 FROM policy_groups
    WHERE id = 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaa3'
)
ON CONFLICT (id) DO NOTHING;
