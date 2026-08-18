export const DEFAULT_WEBHOOK_BODY_TEMPLATE = `[if alert.triggered]
🚨 ALERT TRIGGERED
Event: {event.kind} · {event.id}
Occurred at (unix): {event.occurred_at_unix}
Episode: {alert.id} · generation {alert.trigger_generation}
Record: {alert.record_kind} · lifecycle {alert.lifecycle_state}
Classification: {alert.category} · {alert.severity}
Title: {alert.title}
Detail: {alert.detail}
Policy: {policy.name} ({policy.id})
Rule: {policy_rule.name} ({policy_rule.id})
Target: {alert.target_kind}:{alert.target_id}
[if alert.client_id]Client: {alert.client_id}
[endif]Source status: {alert.source_status}
Triggered at: {alert.triggered_at}
Observed subjects: {matched_vps.map(vps.display_name).join(", ")}
[elseif alert.resolved]
✅ ALERT RESOLVED
Event: {event.kind} · {event.id}
Occurred at (unix): {event.occurred_at_unix}
Episode: {alert.id} · generation {alert.trigger_generation}
Record: {alert.record_kind} · lifecycle {alert.lifecycle_state}
Classification: {alert.category} · {alert.severity}
Title: {alert.title}
Detail: {alert.detail}
Policy: {policy.name} ({policy.id})
Rule: {policy_rule.name} ({policy_rule.id})
Target: {alert.target_kind}:{alert.target_id}
[if alert.client_id]Client: {alert.client_id}
[endif]Source status: {alert.source_status}
Triggered at: {alert.triggered_at}
Resolved at: {alert.resolved_at}
Resolution: {alert.resolution_reason}
[if alert.resolution_note]Resolution note: {alert.resolution_note}
[endif]Observed subjects: {matched_vps.map(vps.display_name).join(", ")}
[elseif event.kind = "schedule.due"]
⏰ SCHEDULE DUE
Event: {event.kind} · {event.id}
Occurred at (unix): {event.occurred_at_unix}
Schedule: {schedule.name} ({schedule.id})
Trigger: {schedule.trigger_kind} · definition revision {schedule.definition_revision}
Command: {schedule.command_type}
Selector snapshot: {schedule.selector_expression}
Catch-up run: {schedule.catch_up_run_index}/{schedule.catch_up_run_count} · {schedule.catch_up_policy}
Job: {job.id} · {job.type} · {job.status}
Target count: {job.target_count}
Target IDs: {schedule.target_ids.join(", ")}
Matched VPSs: {matched_vps.map(vps.display_name).join(", ")}
[elseif event.kind = "schedule.job_finished"]
🏁 SCHEDULE JOB FINISHED
Event: {event.kind} · {event.id}
Occurred at (unix): {event.occurred_at_unix}
Schedule: {schedule.name} ({schedule.id})
Job: {job.id} · {job.type} · {job.status}
Target count: {job.target_count}
Target IDs: {job.target_ids.join(", ")}
[if schedule.last_job_error]Error: {schedule.last_job_error}
[endif]Matched VPSs: {matched_vps.map(vps.display_name).join(", ")}
[elseif event.kind = "job.status"]
🛠️ JOB STATUS
Event: {event.kind} · {event.id}
Occurred at (unix): {event.occurred_at_unix}
Job: {job.id} · {job.type} · {job.status}
Target count: {job.target_count}
Target IDs: {job.target_ids.join(", ")}
Matched VPSs: {matched_vps.map(vps.display_name).join(", ")}
[elseif event.kind = "vps.status_changed"]
🖥️ VPS STATUS CHANGED
Event: {event.kind} · {event.id}
Occurred at (unix): {event.occurred_at_unix}
[if event.from_status]Transition: {event.from_status} → {event.to_status}
[else]Status: {event.to_status}
[endif]Reason: {event.reason}
Affected VPSs: {matched_vps.map(vps.display_name).join(", ")}
[elseif event.kind = "telemetry.rollup"]
📊 TELEMETRY ROLLUP
Event: {event.kind} · {event.id}
Occurred at (unix): {event.occurred_at_unix}
Telemetry subject: {telemetry.client_id} via {telemetry.gateway_id}
Host: {telemetry.hostname}
Observed at (unix): {telemetry.observed_unix}
Uptime seconds: {telemetry.uptime_secs}
Networks: {telemetry.network_count} · tunnels: {telemetry.tunnel_count}
Matched VPSs: {matched_vps.map(vps.display_name).join(", ")}
[else]
ℹ️ EVENT
Event: {event.kind} · {event.id}
Occurred at (unix): {event.occurred_at_unix}
Webhook rule: {rule.name}
Expression: {rule.expression}
Matched VPSs: {matched_vps.map(vps.display_name).join(", ")}
[endif]`;
