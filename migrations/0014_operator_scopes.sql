ALTER TABLE operators
ADD COLUMN scopes JSONB NOT NULL DEFAULT '[]'::jsonb;

ALTER TABLE operators
ADD CONSTRAINT operators_scopes_json_array CHECK (jsonb_typeof(scopes) = 'array');
