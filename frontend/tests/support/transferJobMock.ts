import type { Page } from "@playwright/test";

export async function installTransferJobApiMock(page: Page) {
  await page.addInitScript(() => {
    const previousFetch = window.fetch.bind(window);
    const dynamicJobs: Record<string, unknown> = {};
    const dynamicJobOutputs: Record<string, unknown[]> = {};
    const dynamicArtifacts: Record<string, Uint8Array> = {};
    const transferSessionSizes: Record<string, number> = {};
    const transferSessionHashes: Record<string, string> = {};
    let transferJobCounter = 0;
    const downloadFixtureBytes = new TextEncoder().encode("resumable browser download payload");

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
    const createDynamicJob = (jobId: string, command: string, targetCount: number) => {
      dynamicJobs[jobId] = {
        actor_id: null,
        command_type: command,
        completed_at: "2026-05-31T10:11:00Z",
        created_at: "2026-05-31T10:10:59Z",
        id: jobId,
        payload_hash: "f".repeat(64),
        privileged: true,
        status: "completed",
        target_count: targetCount,
      };
    };
    const createTransferOutputs = async (jobId: string, body: unknown) => {
      const request = body as {
        clients?: string[];
        operation?: {
          type?: string;
          session_id?: string;
          path?: string;
          size_bytes?: number;
          offset?: number;
          max_bytes?: number;
          chunk?: { size_bytes?: number };
        };
      };
      const operation = request.operation;
      const sessionId = operation?.session_id ?? "missing-session";
      const clientIds = request.clients && request.clients.length > 0 ? request.clients : ["agent-sfo-01"];
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

    window.fetch = async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = input instanceof Request ? input.url : String(input);
      const pathname = new URL(url, window.location.href).pathname;
      const method = (init?.method ?? (input instanceof Request ? input.method : "GET")).toUpperCase();
      const outputMatch = pathname.match(/^\/api\/v1\/jobs\/([^/]+)\/outputs$/);
      if (outputMatch && method === "GET" && dynamicJobOutputs[outputMatch[1]]) {
        return jsonResponse(dynamicJobOutputs[outputMatch[1]]);
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
        if (operationType?.startsWith("file_transfer_")) {
          const requests = (window as unknown as { __vpsmanTestRequests?: { jobs: unknown[] } }).__vpsmanTestRequests;
          requests?.jobs.push(body);
          const jobId = nextTransferJobId();
          const targetCount = (body as { clients?: string[] } | null)?.clients?.length ?? 1;
          createDynamicJob(jobId, operationType, targetCount);
          await createTransferOutputs(jobId, body);
          return jsonResponse({ accepted_targets: targetCount, job_id: jobId, status: "accepted" });
        }
      }
      return previousFetch(input, init);
    };
  });
}
