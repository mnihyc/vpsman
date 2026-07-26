ALTER TABLE operator_auth_throttle
    DROP CONSTRAINT operator_auth_throttle_scope_kind_check;

ALTER TABLE operator_auth_throttle
    ADD CONSTRAINT operator_auth_throttle_scope_kind_check
    CHECK (scope_kind IN ('username', 'username_ip', 'ip'));
