import { expect, test } from "@playwright/test";
import { buildPrivilegeForJobOperation, canonicalOperationJson } from "../src/privilege";
import type { JobOperation } from "../src/types";

test("canonical privilege payload omits skipped optional fields", () => {
  const terminalOpen: JobOperation = {
    type: "terminal_open",
    session_id: "61616161-2222-4333-8444-555555555555",
    argv: ["/bin/sh", "-l"],
    cwd: null,
    cols: 120,
    rows: 30,
    idle_timeout_secs: 1800,
    flow_window_bytes: 65536,
  };
  expect(canonicalOperationJson(terminalOpen)).toBe(
    '{"type":"terminal_open","session_id":"61616161-2222-4333-8444-555555555555","argv":["/bin/sh","-l"],"user_policy":"fail","cols":120,"rows":30,"idle_timeout_secs":1800,"flow_window_bytes":65536}',
  );

  const filePush: JobOperation = {
    type: "file_push",
    path: "/tmp/upload.txt",
    mode: 0o640,
    size_bytes: 4,
    sha256_hex: "00".repeat(32),
    data_base64: "dGVzdA==",
    existing_policy: "skip",
    ownership_policy: "fail",
  };
  expect(canonicalOperationJson(filePush)).toBe(
    '{"type":"file_push","path":"/tmp/upload.txt","mode":416,"size_bytes":4,"sha256_hex":"0000000000000000000000000000000000000000000000000000000000000000","data_base64":"dGVzdA=="}',
  );

  const transferStart: JobOperation = {
    type: "file_transfer_start",
    session_id: "61616161-2222-4333-8444-555555555555",
    path: "/tmp/upload.bin",
    mode: 0o640,
    size_bytes: 4,
    sha256_hex: "11".repeat(32),
    chunk_size_bytes: 65536,
    rate_limit_kbps: 0,
    existing_policy: "skip",
    resume_token_hash: "22".repeat(32),
  };
  expect(canonicalOperationJson(transferStart)).toBe(
    '{"type":"file_transfer_start","session_id":"61616161-2222-4333-8444-555555555555","path":"/tmp/upload.bin","mode":416,"size_bytes":4,"sha256_hex":"1111111111111111111111111111111111111111111111111111111111111111","chunk_size_bytes":65536,"rate_limit_kbps":0,"resume_token_hash":"2222222222222222222222222222222222222222222222222222222222222222"}',
  );
  expect(canonicalOperationJson({ ...transferStart, existing_policy: "replace" })).toBe(
    '{"type":"file_transfer_start","session_id":"61616161-2222-4333-8444-555555555555","path":"/tmp/upload.bin","mode":416,"size_bytes":4,"sha256_hex":"1111111111111111111111111111111111111111111111111111111111111111","chunk_size_bytes":65536,"rate_limit_kbps":0,"existing_policy":"replace","resume_token_hash":"2222222222222222222222222222222222222222222222222222222222222222"}',
  );
});

test("canonical restore payload keeps non-skipped null archive fields", () => {
  const restore: JobOperation = {
    type: "restore",
    source_backup_request_id: "11111111-2222-4333-8444-555555555555",
    paths: ["/etc/app.conf"],
    include_config: false,
    destination_root: null,
    archive_base64: null,
    archive_size_bytes: null,
    archive_sha256_hex: null,
    dry_run: false,
    post_restore_argv: [],
  };
  expect(canonicalOperationJson(restore)).toBe(
    '{"type":"restore","source_backup_request_id":"11111111-2222-4333-8444-555555555555","paths":["/etc/app.conf"],"include_config":false,"destination_root":null,"archive_base64":null,"archive_size_bytes":null,"archive_sha256_hex":null}',
  );
});

test("generated privilege assertions carry a request-bound timestamp", async () => {
  const beforeUnix = Math.floor(Date.now() / 1000);
  const built = await buildPrivilegeForJobOperation({
    clientIds: ["agent-sfo-01"],
    commandType: "shell_argv",
    operation: { type: "shell", argv: ["/bin/true"], pty: false },
    privilegeMaterial: {
      superPassword: "local-super-password",
      superSaltHex: "01020304",
    },
    selectorExpression: "id:agent-sfo-01",
    timeoutSecs: 30,
  });
  const afterUnix = Math.floor(Date.now() / 1000);
  const assertion = built.privilegeAssertion;

  expect(assertion.issued_unix).toBeGreaterThanOrEqual(beforeUnix);
  expect(assertion.issued_unix).toBeLessThanOrEqual(afterUnix);
  expect(assertion.expires_unix).toBe(assertion.issued_unix + 300);
  expect(assertion.nonce_hex).toMatch(/^[0-9a-f]{32}$/);
  expect(assertion.assertion_hex).toMatch(/^[0-9a-f]{64}$/);
});
