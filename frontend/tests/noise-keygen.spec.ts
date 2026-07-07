import { expect, test } from "@playwright/test";
import { generateNoiseKeypair } from "../src/noiseKeygen";

test("generates Web Crypto X25519 key material exported through JWK", async () => {
  const keypair = await generateNoiseKeypair();
  expect(keypair.privateKeyHex).toMatch(/^[0-9a-f]{64}$/);
  expect(keypair.publicKeyHex).toMatch(/^[0-9a-f]{64}$/);

  await crypto.subtle.importKey(
    "jwk",
    {
      crv: "X25519",
      d: hexToBase64Url(keypair.privateKeyHex),
      ext: true,
      key_ops: ["deriveBits"],
      kty: "OKP",
      x: hexToBase64Url(keypair.publicKeyHex),
    } satisfies JsonWebKey,
    { name: "X25519" },
    true,
    ["deriveBits"],
  );
  await crypto.subtle.importKey(
    "jwk",
    {
      crv: "X25519",
      ext: true,
      key_ops: [],
      kty: "OKP",
      x: hexToBase64Url(keypair.publicKeyHex),
    } satisfies JsonWebKey,
    { name: "X25519" },
    true,
    [],
  );
});

function hexToBase64Url(hex: string): string {
  const bytes = new Uint8Array(hex.length / 2);
  for (let index = 0; index < bytes.length; index += 1) {
    bytes[index] = Number.parseInt(hex.slice(index * 2, index * 2 + 2), 16);
  }
  return btoa(String.fromCharCode(...bytes))
    .replace(/\+/g, "-")
    .replace(/\//g, "_")
    .replace(/=+$/g, "");
}
