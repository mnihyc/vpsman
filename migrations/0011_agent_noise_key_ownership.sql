DO $$
BEGIN
    IF EXISTS (
        SELECT 1
        FROM clients
        WHERE octet_length(public_key) > 0
        GROUP BY public_key
        HAVING count(*) > 1
    ) THEN
        RAISE EXCEPTION 'duplicate_client_public_keys_detected';
    END IF;

    IF EXISTS (
        SELECT 1
        FROM client_key_revocations
        GROUP BY public_key_sha256_hex
        HAVING count(*) > 1
    ) THEN
        RAISE EXCEPTION 'duplicate_revoked_public_keys_detected';
    END IF;
END
$$;

CREATE UNIQUE INDEX clients_public_key_unique_idx
    ON clients (public_key)
    WHERE octet_length(public_key) > 0;

CREATE UNIQUE INDEX client_key_revocations_public_key_unique_idx
    ON client_key_revocations (public_key_sha256_hex);
