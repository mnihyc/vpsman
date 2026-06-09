/**
 * In-browser X25519 keypair generation for direct gateway agent identity.
 * Uses Web Crypto API — no server round-trip, no dependencies.
 */
export async function generateNoiseKeypair(): Promise<{
  privateKeyHex: string;
  publicKeyHex: string;
}> {
  const keypair = (await crypto.subtle.generateKey(
    { name: "X25519" },
    true,
    ["deriveBits"],
  )) as CryptoKeyPair;

  const privateKeyRaw = await crypto.subtle.exportKey("raw", keypair.privateKey);
  const publicKeyRaw = await crypto.subtle.exportKey("raw", keypair.publicKey);

  return {
    privateKeyHex: bytesToHex(new Uint8Array(privateKeyRaw)),
    publicKeyHex: bytesToHex(new Uint8Array(publicKeyRaw)),
  };
}

function bytesToHex(bytes: Uint8Array): string {
  return Array.from(bytes)
    .map((b) => b.toString(16).padStart(2, "0"))
    .join("");
}
