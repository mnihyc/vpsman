const LOCAL_STORAGE_APP_PREFIX = "vpsman.";
const PRESERVED_LOCAL_STORAGE_KEYS = new Set([
  "vpsman.accessToken",
  "vpsman.authVault",
  "vpsman.privilegeVault",
  "vpsman.refreshToken",
]);

export function clearLocalStorageSelections(): number {
  if (typeof window === "undefined") {
    return 0;
  }
  const keys: string[] = [];
  for (let index = 0; index < window.localStorage.length; index += 1) {
    const key = window.localStorage.key(index);
    if (key?.startsWith(LOCAL_STORAGE_APP_PREFIX) && !PRESERVED_LOCAL_STORAGE_KEYS.has(key)) {
      keys.push(key);
    }
  }
  for (const key of keys) {
    window.localStorage.removeItem(key);
  }
  return keys.length;
}
