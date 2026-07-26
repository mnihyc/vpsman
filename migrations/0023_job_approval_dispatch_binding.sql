ALTER TABLE jobs
    ADD COLUMN approval_id UUID REFERENCES job_approvals(id);

WITH ranked_matches AS (
    SELECT
        job.id AS job_id,
        approval.id AS approval_id,
        row_number() OVER (
            PARTITION BY job.id
            ORDER BY
                CASE approval.status
                    WHEN 'approved' THEN 0
                    WHEN 'pending' THEN 1
                    ELSE 2
                END,
                approval.decided_at DESC NULLS LAST,
                approval.requested_at DESC,
                approval.id DESC
        ) AS match_rank
    FROM jobs job
    JOIN job_approvals approval
      ON approval.job_id = job.id
     AND approval.payload_hash = job.payload_hash
     AND approval.request_fingerprint = job.request_fingerprint
    WHERE job.source_schedule_id IS NULL
)
UPDATE jobs job
SET approval_id = ranked.approval_id
FROM ranked_matches ranked
WHERE ranked.job_id = job.id
  AND ranked.match_rank = 1;

CREATE UNIQUE INDEX jobs_approval_id_idx
    ON jobs (approval_id)
    WHERE approval_id IS NOT NULL;
