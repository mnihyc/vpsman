/**
 * In-browser X25519 keypair generation for direct gateway agent identity.
 * Uses Web Crypto only; private keys are exported as JWK because raw private
 * export is not a valid Web Crypto format for X25519.
 */
export async function generateNoiseKeypair(): Promise<{
  privateKeyHex: string;
  publicKeyHex: string;
}> {
  const subtle = globalThis.crypto?.subtle;
  if (!subtle) {
    throw new Error("Web Crypto is unavailable");
  }

  const keypair = (await subtle.generateKey(
    { name: "X25519" },
    true,
    ["deriveBits"],
  )) as CryptoKeyPair;

  const privateJwk = await subtle.exportKey("jwk", keypair.privateKey);
  const publicJwk = await subtle.exportKey("jwk", keypair.publicKey);
  return {
    privateKeyHex: jwkMemberToHex(privateJwk, "d", "private"),
    publicKeyHex: jwkMemberToHex(publicJwk, "x", "public"),
  };
}

function jwkMemberToHex(
  jwk: JsonWebKey,
  member: "d" | "x",
  label: string,
): string {
  if (jwk.kty !== "OKP" || jwk.crv !== "X25519" || !jwk[member]) {
    throw new Error(`Invalid X25519 ${label} JWK export`);
  }
  const bytes = base64UrlToBytes(jwk[member]);
  if (bytes.length !== 32) {
    throw new Error(`Invalid X25519 ${label} key length`);
  }
  return bytesToHex(bytes);
}

function base64UrlToBytes(value: string): Uint8Array {
  const base64 = value.replace(/-/g, "+").replace(/_/g, "/");
  const padded = base64.padEnd(base64.length + ((4 - base64.length % 4) % 4), "=");
  const binary = globalThis.atob(padded);
  const bytes = new Uint8Array(binary.length);
  for (let index = 0; index < binary.length; index += 1) {
    bytes[index] = binary.charCodeAt(index);
  }
  return bytes;
}

function bytesToHex(bytes: Uint8Array): string {
  return Array.from(bytes)
    .map((b) => b.toString(16).padStart(2, "0"))
    .join("");
}
