import type { Page } from "@playwright/test";
import type { OperatorView } from "../../src/types";

type StoredAuthSession = {
  epoch: string;
  status: "active" | "ended";
  accessToken: string;
  refreshToken: string;
  operator: OperatorView | null;
};

/** Read the committed browser record without importing another app singleton. */
export async function readCanonicalAuthSession(page: Page): Promise<StoredAuthSession | null> {
  return page.evaluate(() => new Promise<StoredAuthSession | null>((resolve, reject) => {
    const opening = indexedDB.open("vpsman.authSession");
    opening.onupgradeneeded = () => {
      opening.transaction?.abort();
      reject(new Error("The application has not created its auth session database."));
    };
    opening.onerror = () => reject(opening.error);
    opening.onsuccess = () => {
      const database = opening.result;
      const transaction = database.transaction("session", "readonly");
      const request = transaction.objectStore("session").get("current");
      transaction.oncomplete = () => {
        database.close();
        resolve(request.result ?? null);
      };
      transaction.onabort = transaction.onerror = () => {
        database.close();
        reject(transaction.error);
      };
    };
  }));
}
