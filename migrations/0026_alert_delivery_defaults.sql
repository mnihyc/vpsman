-- Connectivity changes must not remove a VPS from a built-in metric policy.
-- Change only the predefined groups' selectors; retain their rules and state.
UPDATE public.policy_groups
SET selector_expression = '*'
WHERE id IN (
    'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaa3',
    'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaa4',
    'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaa5',
    'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaa6',
    'c1000000-0000-4000-8000-000000000001'
);

-- Existing configured cooldowns remain unchanged.
ALTER TABLE public.webhook_rules ALTER COLUMN cooldown_secs SET DEFAULT 0;
