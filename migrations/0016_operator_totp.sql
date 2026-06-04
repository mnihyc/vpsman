ALTER TABLE operators
ADD COLUMN IF NOT EXISTS totp_secret_ciphertext_hex TEXT,
ADD COLUMN IF NOT EXISTS totp_secret_nonce_hex TEXT,
ADD COLUMN IF NOT EXISTS totp_secret_salt_hex TEXT;

ALTER TABLE operators
ADD CONSTRAINT operators_totp_secret_hex CHECK (
    (totp_secret_ciphertext_hex IS NULL AND totp_secret_nonce_hex IS NULL AND totp_secret_salt_hex IS NULL)
    OR
    (totp_secret_ciphertext_hex ~ '^[0-9a-f]+$'
     AND totp_secret_nonce_hex ~ '^[0-9a-f]{24}$'
     AND totp_secret_salt_hex ~ '^[0-9a-f]{32}$')
);
