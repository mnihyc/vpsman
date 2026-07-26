const retryKey = "vpsman:boot-retries";
const maxAutomaticReloads = 2;
const startupTimeoutMs = 15_000;
const recovery = document.getElementById("boot-recovery");
const recoveryMessage = document.getElementById("boot-recovery-message");
const reloadButton = document.getElementById("reload-console");
const errorDetailsContainer = document.getElementById("boot-error-details");
const errorDetails = document.getElementById("boot-error");

recovery.hidden = true;
reloadButton.addEventListener("click", (event) => {
  event.preventDefault();
  sessionStorage.removeItem(retryKey);
  window.location.reload();
});

async function startConsole() {
  let timeoutId;
  try {
    await Promise.race([
      import("./main.tsx"),
      new Promise((_, reject) => {
        timeoutId = window.setTimeout(
          () => reject(new Error("Console startup timed out after 15 seconds.")),
          startupTimeoutMs,
        );
      }),
    ]);
    sessionStorage.removeItem(retryKey);
  } catch (error) {
    const attempts = Number.parseInt(
      sessionStorage.getItem(retryKey) ?? "0",
      10,
    );
    if (Number.isFinite(attempts) && attempts < maxAutomaticReloads) {
      sessionStorage.setItem(retryKey, String(attempts + 1));
      window.location.reload();
      return;
    }

    sessionStorage.removeItem(retryKey);
    recoveryMessage.textContent =
      "Startup was interrupted after two automatic retries. No operation was submitted.";
    errorDetails.textContent =
      error instanceof Error ? error.message : String(error);
    errorDetailsContainer.hidden = false;
    recovery.hidden = false;
    recovery.dataset.state = "error";
    reloadButton.focus();
  } finally {
    if (timeoutId !== undefined) {
      window.clearTimeout(timeoutId);
    }
  }
}

void startConsole();
