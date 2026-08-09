\set ON_ERROR_STOP on

BEGIN;

INSERT INTO clients (
    id,
    display_name,
    public_key,
    status,
    agent_version,
    process_incarnation_id,
    os_release,
    arch,
    cpu_model,
    kernel_release,
    virtualization,
    system_reported_at,
    capabilities,
    registration_ip,
    last_ip,
    last_seen_at,
    created_at
)
SELECT
    seed.id,
    seed.display_name,
    decode(repeat(lpad(to_hex(seed.key_seed), 2, '0'), 32), 'hex'),
    'online',
    'review-agent-0.2.27',
    seed.process_incarnation_id::uuid,
    'Debian GNU/Linux 13 (trixie)',
    'x86_64',
    'AMD EPYC 7B13',
    '6.12.38-amd64',
    'KVM',
    now(),
    jsonb_build_object(
        'privilege_mode', 'root',
        'can_attempt_privileged_ops', true,
        'can_apply_process_limits', true,
        'can_manage_runtime_tunnels', true,
        'max_job_timeout_secs', 3600
    ),
    seed.ip::inet,
    seed.ip::inet,
    now(),
    now() - interval '400 days'
FROM (
    VALUES
        (
            'review-total-monthly',
            'Total quota · Monthly',
            11,
            '11111111-1111-4111-8111-111111111111',
            '203.0.113.11'
        ),
        (
            'review-traffic-exceeded',
            'Traffic quota exceeded',
            18,
            '88888888-8888-4888-8888-888888888888',
            '203.0.113.18'
        ),
        (
            'review-rx-yearly',
            'RX quota · Annual',
            12,
            '22222222-2222-4222-8222-222222222222',
            '203.0.113.12'
        ),
        (
            'review-tx-unlimited',
            'TX quota · Unlimited',
            13,
            '33333333-3333-4333-8333-333333333333',
            '203.0.113.13'
        ),
        (
            'review-no-reset',
            'Accumulated archive',
            14,
            '44444444-4444-4444-8444-444444444444',
            '203.0.113.14'
        ),
        (
            'review-empty-rates',
            'Rates intentionally empty',
            15,
            '55555555-5555-4555-8555-555555555555',
            '203.0.113.15'
        ),
        (
            'review-unconfigured',
            'Unconfigured traffic',
            16,
            '66666666-6666-4666-8666-666666666666',
            '203.0.113.16'
        ),
        (
            'review-no-primary',
            'No primary Ping',
            17,
            '77777777-7777-4777-8777-777777777777',
            '203.0.113.17'
        )
) AS seed(id, display_name, key_seed, process_incarnation_id, ip)
ON CONFLICT (id) DO UPDATE SET
    display_name = EXCLUDED.display_name,
    status = EXCLUDED.status,
    agent_version = EXCLUDED.agent_version,
    process_incarnation_id = EXCLUDED.process_incarnation_id,
    os_release = EXCLUDED.os_release,
    arch = EXCLUDED.arch,
    cpu_model = EXCLUDED.cpu_model,
    kernel_release = EXCLUDED.kernel_release,
    virtualization = EXCLUDED.virtualization,
    system_reported_at = EXCLUDED.system_reported_at,
    capabilities = EXCLUDED.capabilities,
    last_ip = EXCLUDED.last_ip,
    last_seen_at = EXCLUDED.last_seen_at;

INSERT INTO tags (id, name, display_order)
VALUES
    ('10000000-0000-4000-8000-000000000001', 'review-monitoring', 10),
    ('10000000-0000-4000-8000-000000000002', 'provider:reviewcloud', 20),
    ('10000000-0000-4000-8000-000000000003', 'country:SG', 30),
    ('10000000-0000-4000-8000-000000000004', 'country:DE', 31),
    ('10000000-0000-4000-8000-000000000005', 'country:US', 32),
    ('10000000-0000-4000-8000-000000000006', 'country:JP', 33),
    ('10000000-0000-4000-8000-000000000007', 'region:sin', 40),
    ('10000000-0000-4000-8000-000000000008', 'region:fra', 41),
    ('10000000-0000-4000-8000-000000000009', 'region:iad', 42),
    ('10000000-0000-4000-8000-000000000010', 'region:nrt', 43)
ON CONFLICT (name) DO UPDATE SET display_order = EXCLUDED.display_order;

INSERT INTO client_tags (client_id, tag_id)
SELECT client.id, tag.id
FROM clients client
CROSS JOIN tags tag
WHERE client.id LIKE 'review-%'
  AND tag.name IN ('review-monitoring', 'provider:reviewcloud')
ON CONFLICT DO NOTHING;

INSERT INTO client_tags (client_id, tag_id)
SELECT assignment.client_id, tag.id
FROM (
    VALUES
        ('review-total-monthly', 'country:SG'),
        ('review-total-monthly', 'region:sin'),
        ('review-traffic-exceeded', 'country:JP'),
        ('review-traffic-exceeded', 'region:nrt'),
        ('review-rx-yearly', 'country:DE'),
        ('review-rx-yearly', 'region:fra'),
        ('review-tx-unlimited', 'country:US'),
        ('review-tx-unlimited', 'region:iad'),
        ('review-no-reset', 'country:JP'),
        ('review-no-reset', 'region:nrt'),
        ('review-empty-rates', 'country:SG'),
        ('review-empty-rates', 'region:sin'),
        ('review-unconfigured', 'country:DE'),
        ('review-unconfigured', 'region:fra'),
        ('review-no-primary', 'country:US'),
        ('review-no-primary', 'region:iad')
) AS assignment(client_id, tag_name)
JOIN tags tag ON tag.name = assignment.tag_name
ON CONFLICT DO NOTHING;

INSERT INTO vps_rule_values (
    client_id,
    key,
    value_raw,
    value_json,
    source_kind,
    updated_at
)
VALUES
    (
        'review-total-monthly',
        'traffic.reset_day',
        '1',
        '{"day":1}'::jsonb,
        'review_fixture',
        now()
    ),
    (
        'review-total-monthly',
        'traffic.selectors',
        'eth0',
        '{"selectors":[{"source":"host","interface":"eth0","direction":"total","canonical":"eth0"}]}'::jsonb,
        'review_fixture',
        now()
    ),
    (
        'review-total-monthly',
        'traffic.quota.total',
        '100 GB',
        '{"bytes":100000000000}'::jsonb,
        'review_fixture',
        now()
    ),
    (
        'review-total-monthly',
        'network.port_speed',
        '1 Gbps',
        '{"bps":1000000000,"display":"1 Gbps"}'::jsonb,
        'review_fixture',
        now()
    ),
    (
        'review-total-monthly',
        'billing.price',
        '29.90 ¥/m',
        '{"disabled":false,"price":"29.90","currency":"CNY","currency_display":"¥","period":"month","period_code":"m","display":"29.90 ¥/m"}'::jsonb,
        'review_fixture',
        now()
    ),
    (
        'review-total-monthly',
        'billing.cycle',
        '14',
        '{"display":"14"}'::jsonb,
        'review_fixture',
        now()
    ),
    (
        'review-traffic-exceeded',
        'traffic.reset_day',
        '1',
        '{"day":1}'::jsonb,
        'review_fixture',
        now()
    ),
    (
        'review-traffic-exceeded',
        'traffic.selectors',
        'eth0',
        '{"selectors":[{"source":"host","interface":"eth0","direction":"total","canonical":"eth0"}]}'::jsonb,
        'review_fixture',
        now()
    ),
    (
        'review-traffic-exceeded',
        'traffic.quota.total',
        '10 GB',
        '{"bytes":10000000000}'::jsonb,
        'review_fixture',
        now()
    ),
    (
        'review-rx-yearly',
        'traffic.reset_day',
        '1',
        '{"day":1}'::jsonb,
        'review_fixture',
        now()
    ),
    (
        'review-rx-yearly',
        'traffic.selectors',
        'eth0+rx',
        '{"selectors":[{"source":"host","interface":"eth0","direction":"rx","canonical":"eth0+rx"}]}'::jsonb,
        'review_fixture',
        now()
    ),
    (
        'review-rx-yearly',
        'traffic.quota.rx',
        '50 GB',
        '{"bytes":50000000000}'::jsonb,
        'review_fixture',
        now()
    ),
    (
        'review-rx-yearly',
        'billing.price',
        '120.00 USD/y',
        '{"disabled":false,"price":"120.00","currency":"USD","currency_display":"USD","period":"year","period_code":"y","display":"120.00 USD/y"}'::jsonb,
        'review_fixture',
        now()
    ),
    (
        'review-rx-yearly',
        'billing.cycle',
        '15-06',
        '{"display":"15-06"}'::jsonb,
        'review_fixture',
        now()
    ),
    (
        'review-tx-unlimited',
        'traffic.reset_day',
        '1',
        '{"day":1}'::jsonb,
        'review_fixture',
        now()
    ),
    (
        'review-tx-unlimited',
        'traffic.selectors',
        'eth0+tx',
        '{"selectors":[{"source":"host","interface":"eth0","direction":"tx","canonical":"eth0+tx"}]}'::jsonb,
        'review_fixture',
        now()
    ),
    (
        'review-tx-unlimited',
        'traffic.quota.tx',
        '-1',
        '{"bytes":-1}'::jsonb,
        'review_fixture',
        now()
    ),
    (
        'review-tx-unlimited',
        'billing.price',
        '-1',
        '{"disabled":true,"display":"-"}'::jsonb,
        'review_fixture',
        now()
    ),
    (
        'review-no-reset',
        'traffic.reset_day',
        '-1',
        '{"day":-1}'::jsonb,
        'review_fixture',
        now()
    ),
    (
        'review-no-reset',
        'traffic.selectors',
        'eth0',
        '{"selectors":[{"source":"host","interface":"eth0","direction":"total","canonical":"eth0"}]}'::jsonb,
        'review_fixture',
        now()
    ),
    (
        'review-no-reset',
        'traffic.quota.total',
        '200 GB',
        '{"bytes":200000000000}'::jsonb,
        'review_fixture',
        now()
    ),
    (
        'review-empty-rates',
        'traffic.reset_day',
        '1',
        '{"day":1}'::jsonb,
        'review_fixture',
        now()
    ),
    (
        'review-empty-rates',
        'traffic.selectors',
        'tunnel:wg0',
        '{"selectors":[{"source":"tunnel","interface":"wg0","direction":"total","canonical":"tunnel:wg0"}]}'::jsonb,
        'review_fixture',
        now()
    ),
    (
        'review-empty-rates',
        'traffic.quota.total',
        '25 GB',
        '{"bytes":25000000000}'::jsonb,
        'review_fixture',
        now()
    ),
    (
        'review-no-primary',
        'traffic.reset_day',
        '1',
        '{"day":1}'::jsonb,
        'review_fixture',
        now()
    ),
    (
        'review-no-primary',
        'traffic.selectors',
        'eth0',
        '{"selectors":[{"source":"host","interface":"eth0","direction":"total","canonical":"eth0"}]}'::jsonb,
        'review_fixture',
        now()
    ),
    (
        'review-no-primary',
        'traffic.quota.total',
        '80 GB',
        '{"bytes":80000000000}'::jsonb,
        'review_fixture',
        now()
    )
ON CONFLICT (client_id, key) DO UPDATE SET
    value_raw = EXCLUDED.value_raw,
    value_json = EXCLUDED.value_json,
    source_kind = EXCLUDED.source_kind,
    updated_at = EXCLUDED.updated_at;

INSERT INTO ping_targets (
    id,
    name,
    host,
    probe_kind,
    port,
    enabled,
    selector_expression,
    generation,
    created_at,
    updated_at
)
VALUES
    (
        '20000000-0000-4000-8000-000000000001',
        'Review healthy gateway',
        'healthy.review.invalid',
        'icmp',
        NULL,
        true,
        'id:review-total-monthly',
        1,
        now(),
        now()
    ),
    (
        '20000000-0000-4000-8000-000000000002',
        'Review degraded gateway',
        'degraded.review.invalid',
        'icmp',
        NULL,
        true,
        'id:review-rx-yearly',
        1,
        now(),
        now()
    )
ON CONFLICT (id) DO UPDATE SET
    name = EXCLUDED.name,
    host = EXCLUDED.host,
    probe_kind = EXCLUDED.probe_kind,
    port = EXCLUDED.port,
    enabled = EXCLUDED.enabled,
    selector_expression = EXCLUDED.selector_expression,
    generation = EXCLUDED.generation,
    updated_at = EXCLUDED.updated_at;

INSERT INTO ping_target_assignments (target_id, client_id, is_primary, assigned_at)
VALUES
    (
        '20000000-0000-4000-8000-000000000001',
        'review-total-monthly',
        true,
        now()
    ),
    (
        '20000000-0000-4000-8000-000000000001',
        'review-traffic-exceeded',
        true,
        now()
    ),
    (
        '20000000-0000-4000-8000-000000000002',
        'review-rx-yearly',
        true,
        now()
    )
ON CONFLICT (target_id, client_id) DO UPDATE SET
    is_primary = EXCLUDED.is_primary,
    assigned_at = EXCLUDED.assigned_at;

COMMIT;
