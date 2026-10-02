-- Accepted jobs keep their exact targets, independently of the new floor policy.
ALTER TABLE public.tunnel_plans
    ADD COLUMN left_desired_ospf_cost integer CHECK (left_desired_ospf_cost BETWEEN 1 AND 65535),
    ADD COLUMN right_desired_ospf_cost integer CHECK (right_desired_ospf_cost BETWEEN 1 AND 65535);

UPDATE public.tunnel_plans
SET left_desired_ospf_cost = desired_ospf_cost,
    right_desired_ospf_cost = desired_ospf_cost
WHERE desired_ospf_cost IS NOT NULL;

-- Neutral endpoint adjustments preserve the existing dynamic calculation. The
-- shared default floor is deliberately 5 for existing and newly created plans.
-- The v0.5.34 predecessor has no directional fields, so its preview backfill
-- uses neutral adjustments and the existing integer base (no formula change).
-- Leave OSPF-null records alone and preserve modes, bounds, adapters and evidence.
-- The normal plan-update triggers publish the changed runtime evidence identity;
-- no link deletion, observation cleanup or pending-job reset is performed here.
UPDATE public.tunnel_plans
SET input = CASE WHEN jsonb_typeof(input->'ospf') = 'object' THEN
        jsonb_set(input, '{ospf}',
            '{"left_cost_offset":0,"right_cost_offset":0,"left_cost_multiplier":1,"right_cost_multiplier":1,"cost_floor":5}'::jsonb
            || (input->'ospf')) ELSE input END,
    plan = CASE WHEN jsonb_typeof(plan->'ospf') = 'object' THEN
        jsonb_set(plan, '{ospf}',
            '{"left_cost_offset":0,"right_cost_offset":0,"left_cost_multiplier":1,"right_cost_multiplier":1,"cost_floor":5}'::jsonb
            || (plan->'ospf'))
        || jsonb_build_object(
            'left_recommended_ospf_cost', CASE WHEN plan->>'recommended_ospf_cost' IS NULL THEN NULL ELSE GREATEST(
                COALESCE((plan #>> '{ospf,policy,min_cost}')::integer, 5),
                LEAST(COALESCE((plan #>> '{ospf,policy,max_cost}')::integer, 65535),
                    ((plan->>'recommended_ospf_cost')::integer / 5) * 5)) END,
            'right_recommended_ospf_cost', CASE WHEN plan->>'recommended_ospf_cost' IS NULL THEN NULL ELSE GREATEST(
                COALESCE((plan #>> '{ospf,policy,min_cost}')::integer, 5),
                LEAST(COALESCE((plan #>> '{ospf,policy,max_cost}')::integer, 65535),
                    ((plan->>'recommended_ospf_cost')::integer / 5) * 5)) END)
        ELSE plan END,
    revision = revision + 1
WHERE jsonb_typeof(input->'ospf') = 'object' OR jsonb_typeof(plan->'ospf') = 'object';
