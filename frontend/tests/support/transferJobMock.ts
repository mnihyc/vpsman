import type { Page } from "@playwright/test";

export async function installTransferJobApiMock(page: Page) {
  await page.addInitScript(() => {
    const previousFetch = window.fetch.bind(window);
    const dynamicJobs: Record<string, unknown> = {};
    const dynamicJobOutputs: Record<string, unknown[]> = {};
    const dynamicJobTargets: Record<string, unknown[]> = {};
    const dynamicArtifacts: Record<string, Uint8Array> = {};
    const transferSessionSizes: Record<string, number> = {};
    const transferSessionHashes: Record<string, string> = {};
    let transferJobCounter = 0;
    const downloadFixtureBytes = new TextEncoder().encode("resumable browser download payload");
    const selectorAgents = [
      { display_name: "edge-sfo-01", id: "agent-sfo-01", status: "online", tags: ["provider:alpha", "country:US", "edge"] },
      { display_name: "core-fra-02", id: "agent-fra-02", status: "online", tags: ["country:DE", "bgp", "bird2"] },
      { display_name: "backup-nyc-03", id: "agent-nyc-03", status: "stale", tags: ["country:US"] },
    ];
    type SelectorAgent = (typeof selectorAgents)[number];
    type SelectorToken = { kind: "and" | "left" | "or" | "right" } | { kind: "term"; raw: string };
    type SelectorExpr =
      | { type: "term"; raw: string }
      | { type: "and"; left: SelectorExpr; right: SelectorExpr }
      | { type: "or"; left: SelectorExpr; right: SelectorExpr };
    const valueMatches = (value: string, pattern: string, contains: boolean) => {
      const normalizedValue = value.toLocaleLowerCase();
      const normalizedPattern = pattern.toLocaleLowerCase();
      if (normalizedPattern.includes("*") || normalizedPattern.includes("?")) {
        const regex = new RegExp(
          `^${normalizedPattern.replace(/[.+^${}()|[\]\\]/g, "\\$&").replace(/\*/g, ".*").replace(/\?/g, ".")}$`,
        );
        return regex.test(normalizedValue);
      }
      return contains ? normalizedValue.includes(normalizedPattern) : normalizedValue === normalizedPattern;
    };
    const tokenizeSelectorExpression = (expression: string): SelectorToken[] => {
      const tokens: SelectorToken[] = [];
      let index = 0;
      while (index < expression.length) {
        const char = expression[index];
        if (/\s/.test(char)) {
          index += 1;
          continue;
        }
        if (char === "(" || char === ")") {
          tokens.push({ kind: char === "(" ? "left" : "right" });
          index += 1;
          continue;
        }
        if (char === "&" || char === "|") {
          if (expression[index + 1] !== char) {
            throw new Error("Use && or || for boolean operators");
          }
          tokens.push({ kind: char === "&" ? "and" : "or" });
          index += 2;
          continue;
        }
        const start = index;
        while (index < expression.length && !/[\s()&|]/.test(expression[index])) {
          index += 1;
        }
        const raw = expression.slice(start, index);
        const lower = raw.toLocaleLowerCase();
        tokens.push(lower === "and" || lower === "or" ? { kind: lower === "and" ? "and" : "or" } : { kind: "term", raw });
      }
      return tokens;
    };
    const parseSelectorExpression = (expression: string): SelectorExpr | null => {
      const tokens = tokenizeSelectorExpression(expression);
      if (tokens.length === 0) {
        return null;
      }
      let position = 0;
      const peek = () => tokens[position];
      const consume = () => tokens[position++];
      const startsPrimary = () => {
        const token = peek();
        return token?.kind === "term" || token?.kind === "left";
      };
      const parsePrimary = (): SelectorExpr => {
        const token = consume();
        if (!token) {
          throw new Error("Expression is incomplete");
        }
        if (token.kind === "term") {
          return { type: "term", raw: token.raw };
        }
        if (token.kind === "left") {
          const nested = parseOr();
          if (consume()?.kind !== "right") {
            throw new Error("Missing closing parenthesis");
          }
          return nested;
        }
        throw new Error("Operator is missing an operand");
      };
      const parseAnd = (): SelectorExpr => {
        let current = parsePrimary();
        while (peek()?.kind === "and" || startsPrimary()) {
          if (peek()?.kind === "and") {
            consume();
          }
          current = { type: "and", left: current, right: parsePrimary() };
        }
        return current;
      };
      const parseOr = (): SelectorExpr => {
        let current = parseAnd();
        while (peek()?.kind === "or") {
          consume();
          current = { type: "or", left: current, right: parseAnd() };
        }
        return current;
      };
      const parsed = parseOr();
      if (position < tokens.length) {
        throw new Error("Unexpected token after expression");
      }
      return parsed;
    };
    const termMatchesAgent = (agent: SelectorAgent, term: string) => {
      const separator = term.indexOf(":");
      if (separator > 0) {
        const namespace = term.slice(0, separator).toLocaleLowerCase();
        const value = term.slice(separator + 1);
        if (!value) {
          return false;
        }
        if (namespace === "id") {
          return valueMatches(agent.id, value, false);
        }
        if (namespace === "name") {
          return valueMatches(agent.display_name, value, false);
        }
        if (namespace === "tag") {
          return agent.tags.some((tag) => valueMatches(tag, value, false));
        }
        if (namespace === "provider") {
          return agent.tags.some((tag) => valueMatches(tag, `provider:${value}`, false));
        }
        if (namespace === "country" || namespace === "region") {
          return agent.tags.some((tag) => valueMatches(tag, `country:${value}`, false));
        }
        if (namespace === "status") {
          return valueMatches(agent.status, value, false);
        }
        return false;
      }
      return valueMatches(agent.id, term, true) || valueMatches(agent.display_name, term, true);
    };
    const evaluateSelectorExpression = (agent: SelectorAgent, expression: SelectorExpr | null): boolean => {
      if (!expression) {
        return true;
      }
      if (expression.type === "and") {
        return evaluateSelectorExpression(agent, expression.left) && evaluateSelectorExpression(agent, expression.right);
      }
      if (expression.type === "or") {
        return evaluateSelectorExpression(agent, expression.left) || evaluateSelectorExpression(agent, expression.right);
      }
      return termMatchesAgent(agent, expression.raw);
    };
    const clientIdsFromSelector = (selectorExpression: string | undefined): string[] => {
      const expression = selectorExpression?.trim();
      if (!expression) {
        return ["agent-sfo-01"];
      }
      let parsed: SelectorExpr | null = null;
      try {
        parsed = parseSelectorExpression(expression);
      } catch {
        return ["agent-sfo-01"];
      }
      const ids = selectorAgents
        .filter((agent) => evaluateSelectorExpression(agent, parsed))
        .map((agent) => agent.id);
      return ids.length > 0 ? ids : ["agent-sfo-01"];
    };
    const selectorAgentById = new Map(selectorAgents.map((agent) => [agent.id, agent]));
    const onlineClientIds = (clientIds: string[]) =>
      clientIds.filter((clientId) => selectorAgentById.get(clientId)?.status === "online");
    const dispatchableClientIds = (clientIds: string[]) =>
      clientIds.filter((clientId) => selectorAgentById.get(clientId)?.status !== "offline");

    const jsonResponse = (body: unknown) =>
      Promise.resolve(
        new Response(JSON.stringify(body), {
          headers: { "Content-Type": "application/json" },
          status: 200,
        }),
      );
    const readJsonBody = async (input: RequestInfo | URL, init?: RequestInit) => {
      const body = init?.body;
      if (typeof body === "string") {
        return JSON.parse(body) as unknown;
      }
      if (input instanceof Request) {
        return input.clone().json() as Promise<unknown>;
      }
      return null;
    };
    const statusOutputBody = (value: unknown) => btoa(JSON.stringify(value));
    const bytesToBase64 = (bytes: Uint8Array) => {
      let binary = "";
      for (const byte of bytes) {
        binary += String.fromCharCode(byte);
      }
      return btoa(binary);
    };
    const bytesToHex = (bytes: Uint8Array) =>
      Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join("");
    const sha256Hex = async (bytes: Uint8Array) => bytesToHex(new Uint8Array(await crypto.subtle.digest("SHA-256", bytes)));
    const nextTransferJobId = () => {
      const ids = [
        "22222222-3333-4444-8555-666666666666",
        "33333333-4444-4555-8666-777777777777",
        "44444444-5555-4666-8777-888888888888",
        "55555555-6666-4777-8888-999999999999",
        "66666666-7777-4888-8999-aaaaaaaaaaaa",
      ];
      const id = ids[transferJobCounter] ?? `77777777-8888-4999-8aaa-${String(transferJobCounter).padStart(12, "0")}`;
      transferJobCounter += 1;
      return id;
    };
    const createDynamicJob = (jobId: string, command: string, targetCount: number, status = "completed") => {
      dynamicJobs[jobId] = {
        actor_id: null,
        command_type: command,
        completed_at: "2026-05-31T10:11:00Z",
        created_at: "2026-05-31T10:10:59Z",
        id: jobId,
        payload_hash: "f".repeat(64),
        privileged: true,
        status,
        target_count: targetCount,
      };
    };
    const createDynamicJobTargets = (jobId: string, clientIds: string[], outputClientIds: string[]) => {
      const outputClientSet = new Set(outputClientIds);
      dynamicJobTargets[jobId] = clientIds.map((clientId) => {
        const completed = outputClientSet.has(clientId);
        const agentStatus = selectorAgentById.get(clientId)?.status;
        const stale = agentStatus === "stale";
        return {
          client_id: clientId,
          completed_at: "2026-05-31T10:11:00Z",
          exit_code: completed ? 0 : stale ? 2 : null,
          job_id: jobId,
          message: completed ? "completed" : stale ? "stale: file operation command version mismatch" : "agent offline",
          started_at: completed || stale ? "2026-05-31T10:10:59Z" : null,
          status: completed ? "completed" : stale ? "failed" : "dispatch_failed",
        };
      });
    };
    const createTransferOutputs = async (jobId: string, body: unknown) => {
      const request = body as {
        operation?: {
          type?: string;
          session_id?: string;
          path?: string;
          size_bytes?: number;
          offset?: number;
          max_bytes?: number;
          chunk?: { size_bytes?: number };
        };
        selector_expression?: string;
      };
      const operation = request.operation;
      const sessionId = operation?.session_id ?? "missing-session";
      const clientIds = clientIdsFromSelector(request.selector_expression);
      const clientId = clientIds[0];
      let status: unknown = null;
      if (operation?.type === "file_transfer_start") {
        transferSessionSizes[sessionId] = operation.size_bytes ?? 0;
        status = {
          type: "file_transfer_start",
          session_id: sessionId,
          path: operation.path,
          next_offset: 0,
          size_bytes: operation.size_bytes ?? 0,
        };
      }
      if (operation?.type === "file_transfer_download_start") {
        transferSessionSizes[sessionId] = downloadFixtureBytes.length;
        transferSessionHashes[sessionId] = await sha256Hex(downloadFixtureBytes);
        status = {
          type: "file_transfer_download_start",
          session_id: sessionId,
          path: operation.path,
          next_offset: 0,
          size_bytes: downloadFixtureBytes.length,
          extra: {
            sha256_hex: transferSessionHashes[sessionId],
          },
        };
      }
      if (operation?.type === "file_transfer_chunk") {
        const nextOffset = (operation.offset ?? 0) + (operation.chunk?.size_bytes ?? 0);
        status = {
          type: "file_transfer_chunk_ack",
          session_id: sessionId,
          next_offset: nextOffset,
          size_bytes: transferSessionSizes[sessionId] ?? nextOffset,
        };
      }
      if (operation?.type === "file_transfer_download_chunk") {
        const offset = operation.offset ?? 0;
        const maxBytes = operation.max_bytes ?? 65536;
        const chunk = downloadFixtureBytes.subarray(offset, Math.min(offset + maxBytes, downloadFixtureBytes.length));
        const nextOffset = offset + chunk.length;
        const chunkHash = await sha256Hex(chunk);
        status = {
          type: "file_transfer_download_chunk",
          session_id: sessionId,
          next_offset: nextOffset,
          size_bytes: downloadFixtureBytes.length,
          extra: {
            chunk_sha256_hex: chunkHash,
            complete: nextOffset === downloadFixtureBytes.length,
            file_sha256_hex: transferSessionHashes[sessionId] ?? await sha256Hex(downloadFixtureBytes),
            offset,
          },
        };
        dynamicArtifacts[`${jobId}/${clientId}/1`] = chunk;
      }
      if (operation?.type === "file_transfer_commit") {
        status = {
          type: "file_transfer_commit",
          session_id: sessionId,
          next_offset: transferSessionSizes[sessionId] ?? 0,
          size_bytes: transferSessionSizes[sessionId] ?? 0,
        };
      }
      if (!status) {
        return;
      }
      dynamicJobOutputs[jobId] = clientIds.map((outputClientId, index) => ({
        client_id: outputClientId,
        created_at: "2026-05-31T10:11:00Z",
        data_base64: statusOutputBody(status),
        done: true,
        exit_code: 0,
        job_id: jobId,
        seq: index,
        stream: "status",
      }));
      if (operation?.type === "file_transfer_download_chunk") {
        dynamicJobOutputs[jobId] = [
          {
            client_id: clientId,
            created_at: "2026-05-31T10:11:00Z",
            data_base64: statusOutputBody(status),
          done: true,
          exit_code: 0,
          job_id: jobId,
            seq: 0,
            stream: "status",
          },
        ];
        const chunk = dynamicArtifacts[`${jobId}/${clientId}/1`];
        dynamicJobOutputs[jobId].push({
          artifact_sha256_hex: await sha256Hex(chunk),
          artifact_size_bytes: chunk.length,
          client_id: clientId,
          created_at: "2026-05-31T10:11:00Z",
          data_base64: "",
          done: false,
          exit_code: null,
          job_id: jobId,
          seq: 1,
          storage: "object_store",
          stream: "stdout",
        });
      }
    };
    const createFileBrowserOutputs = async (jobId: string, body: unknown) => {
      const request = body as {
        operation?: {
          type?: string;
          path?: string;
          new_path?: string;
          mode?: number;
          existing_policy?: string;
          ownership_policy?: string;
          owner?: string | null;
          group?: string | null;
          content_base64?: string;
          size_bytes?: number;
          sha256_hex?: string;
          recursive?: boolean;
          policy?: string;
        };
        selector_expression?: string;
      };
      const operation = request.operation;
      const selectedClientIds = clientIdsFromSelector(request.selector_expression);
      const clientIds = onlineClientIds(selectedClientIds);
      if (!operation?.type) {
        return;
      }
      const baseMetadata = (path: string, kind: "directory" | "file", size = 0) => ({
        file_type: kind,
        gid: 0,
        is_dir: kind === "directory",
        is_file: kind === "file",
        is_symlink: false,
        mode: kind === "directory" ? 0o755 : 0o644,
        mtime_unix: 1780000000,
        path,
        size_bytes: size,
        symlink_target: null,
        uid: 0,
      });
      const statusForClient = async (clientId: string) => {
        const path = operation.path ?? "/";
        if (operation.type === "file_list_dir") {
          return {
            entries: [
              { ...baseMetadata("/etc", "directory"), name: "etc" },
              { ...baseMetadata("/var", "directory"), name: "var" },
              { ...baseMetadata("/etc/app.conf", "file", 16), name: "app.conf" },
            ],
            limit: 250,
            metadata: baseMetadata(path, "directory"),
            offset: 0,
            path,
            total_entries: 3,
            truncated: false,
            type: "file_list_dir",
          };
        }
        if (operation.type === "file_read_text") {
          const text = "server shared\nlisten=443\n";
          const bytes = new TextEncoder().encode(text);
          return {
            content_base64: bytesToBase64(bytes),
            metadata: baseMetadata(path, "file", bytes.length),
            path,
            sha256_hex: await sha256Hex(bytes),
            size_bytes: bytes.length,
            truncated: false,
            type: "file_read_text",
          };
        }
        if (operation.type === "file_download") {
          const isDirectory = path.endsWith("/") || path === "/";
          const data = new TextEncoder().encode(isDirectory ? "archive" : "download");
          return {
            archive: isDirectory,
            content_type: isDirectory ? "application/x-tar" : "application/octet-stream",
            filename: isDirectory ? "etc.tar" : "app.conf",
            path,
            sha256_hex: await sha256Hex(data),
            size_bytes: data.length,
            source_kind: isDirectory ? "directory" : "file",
            status: "completed",
            type: "file_download",
          };
        }
        if (operation.type === "file_push" || operation.type === "file_push_chunked") {
          return {
            gid: 0,
            group: operation.group ?? null,
            mode: operation.mode,
            ownership_status: operation.owner || operation.group ? "applied" : "unchanged",
            overwrite_policy: operation.existing_policy ?? "skip",
            path,
            sha256_hex: operation.sha256_hex,
            size_bytes: operation.size_bytes,
            status: "completed",
            type: operation.type,
            uid: 0,
            owner: operation.owner ?? null,
          };
        }
        return {
          mode: operation.mode,
          new_path: operation.new_path,
          path,
          policy: operation.policy,
          recursive: operation.recursive,
          sha256_hex: operation.sha256_hex,
          size_bytes: operation.size_bytes,
          status: operation.policy === "ensure" ? "unchanged" : "completed",
          type: operation.type,
        };
      };
      const outputs = [];
      for (const [index, clientId] of clientIds.entries()) {
        const status = await statusForClient(clientId);
        outputs.push({
          client_id: clientId,
          created_at: "2026-05-31T10:11:00Z",
          data_base64: statusOutputBody(status),
          done: true,
          exit_code: 0,
          job_id: jobId,
          seq: index,
          stream: "status",
        });
        if (operation.type === "file_download") {
          const downloadPath = operation.path ?? "/";
          outputs.push({
            client_id: clientId,
            created_at: "2026-05-31T10:11:00Z",
            data_base64: bytesToBase64(new TextEncoder().encode(downloadPath.endsWith("/") || downloadPath === "/" ? "archive" : "download")),
            done: false,
            exit_code: null,
            job_id: jobId,
            seq: index + 100,
            stream: "stdout",
          });
        }
      }
      dynamicJobOutputs[jobId] = outputs;
    };

    window.fetch = async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = input instanceof Request ? input.url : String(input);
      const pathname = new URL(url, window.location.href).pathname;
      const method = (init?.method ?? (input instanceof Request ? input.method : "GET")).toUpperCase();
      const outputMatch = pathname.match(/^\/api\/v1\/jobs\/([^/]+)\/outputs$/);
      if (outputMatch && method === "GET" && dynamicJobOutputs[outputMatch[1]]) {
        return jsonResponse(dynamicJobOutputs[outputMatch[1]]);
      }
      const targetMatch = pathname.match(/^\/api\/v1\/jobs\/([^/]+)\/targets$/);
      if (targetMatch && method === "GET" && dynamicJobTargets[targetMatch[1]]) {
        return jsonResponse(dynamicJobTargets[targetMatch[1]]);
      }
      const bundleMatch = pathname.match(/^\/api\/v1\/jobs\/([^/]+)\/outputs\/download-bundle$/);
      if (bundleMatch && method === "GET" && dynamicJobOutputs[bundleMatch[1]]) {
        return Promise.resolve(
          new Response(new TextEncoder().encode("server-side tar bundle"), {
            headers: { "Content-Type": "application/x-tar" },
            status: 200,
          }),
        );
      }
      const artifactMatch = pathname.match(/^\/api\/v1\/jobs\/([^/]+)\/outputs\/([^/]+)\/(\d+)\/artifact$/);
      if (artifactMatch && method === "GET") {
        const key = `${artifactMatch[1]}/${decodeURIComponent(artifactMatch[2])}/${artifactMatch[3]}`;
        const bytes = dynamicArtifacts[key];
        if (bytes) {
          return Promise.resolve(new Response(bytes, { headers: { "Content-Type": "application/octet-stream" }, status: 200 }));
        }
      }
      const jobMatch = pathname.match(/^\/api\/v1\/jobs\/([^/]+)$/);
      if (jobMatch && method === "GET" && dynamicJobs[jobMatch[1]]) {
        return jsonResponse(dynamicJobs[jobMatch[1]]);
      }
      if (pathname === "/api/v1/jobs" && method === "POST") {
        const body = await readJsonBody(input, init);
        const operationType = (body as { operation?: { type?: string } } | null)?.operation?.type;
        if (
          operationType &&
          [
            "file_archive_tar",
            "file_chmod",
            "file_chown",
            "file_copy",
            "file_delete",
            "file_download",
            "file_list_dir",
            "file_mkdir",
            "file_push",
            "file_push_chunked",
            "file_read_text",
            "file_rename",
            "file_write_text",
          ].includes(operationType)
        ) {
          const requests = (window as unknown as { __vpsmanTestRequests?: { fileBrowserJobs?: unknown[]; jobs: unknown[] } }).__vpsmanTestRequests;
          requests?.jobs.push(body);
          requests?.fileBrowserJobs?.push({
            operation: {
              expected_sha256_hex: (body as { operation?: { expected_sha256_hex?: string } } | null)?.operation?.expected_sha256_hex,
              create: (body as { operation?: { create?: boolean } } | null)?.operation?.create,
              existing_policy: (body as { operation?: { existing_policy?: string } } | null)?.operation?.existing_policy,
              mode: (body as { operation?: { mode?: number } } | null)?.operation?.mode,
              new_path: (body as { operation?: { new_path?: string } } | null)?.operation?.new_path,
              ownership_policy: (body as { operation?: { ownership_policy?: string } } | null)?.operation?.ownership_policy,
              path: (body as { operation?: { path?: string } } | null)?.operation?.path,
              policy: (body as { operation?: { policy?: string } } | null)?.operation?.policy,
              recursive: (body as { operation?: { recursive?: boolean } } | null)?.operation?.recursive,
              size_bytes: (body as { operation?: { size_bytes?: number } } | null)?.operation?.size_bytes,
              type: operationType,
            },
            selector_expression: (body as { selector_expression?: string } | null)?.selector_expression,
          });
          const jobId = nextTransferJobId();
          const selectedClientIds = clientIdsFromSelector(
            (body as { selector_expression?: string } | null)?.selector_expression,
          );
          const outputClientIds = selectedClientIds.filter((clientId) => selectorAgentById.get(clientId)?.status === "online");
          const status = outputClientIds.length === selectedClientIds.length ? "completed" : "partially_completed";
          createDynamicJob(jobId, operationType, selectedClientIds.length, status);
          await createFileBrowserOutputs(jobId, body);
          createDynamicJobTargets(jobId, selectedClientIds, outputClientIds);
          return jsonResponse({
            accepted_targets: dispatchableClientIds(selectedClientIds).length,
            target_count: selectedClientIds.length,
            job_id: jobId,
            status: "accepted",
          });
        }
        if (operationType?.startsWith("file_transfer_")) {
          const requests = (window as unknown as { __vpsmanTestRequests?: { jobs: unknown[] } }).__vpsmanTestRequests;
          requests?.jobs.push(body);
          const jobId = nextTransferJobId();
          const targetCount = clientIdsFromSelector(
            (body as { selector_expression?: string } | null)?.selector_expression,
          ).length;
          createDynamicJob(jobId, operationType, targetCount);
          await createTransferOutputs(jobId, body);
          return jsonResponse({
            accepted_targets: targetCount,
            target_count: targetCount,
            job_id: jobId,
            status: "accepted",
          });
        }
      }
      return previousFetch(input, init);
    };
  });
}
