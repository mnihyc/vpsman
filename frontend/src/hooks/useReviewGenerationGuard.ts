import { useCallback, useRef } from "react";

export function useReviewGenerationGuard() {
  const generationRef = useRef(0);

  const captureReviewGeneration = useCallback(() => generationRef.current, []);
  const invalidateReviewGeneration = useCallback(() => {
    generationRef.current += 1;
  }, []);
  const isReviewGenerationCurrent = useCallback(
    (generation: number) => generationRef.current === generation,
    [],
  );

  return {
    captureReviewGeneration,
    invalidateReviewGeneration,
    isReviewGenerationCurrent,
  };
}

const MIN_REVIEW_RENDER_DELAY_MS = 120;

export function waitForReviewRender(): Promise<void> {
  return new Promise((resolve) => {
    const finish = () => window.setTimeout(resolve, MIN_REVIEW_RENDER_DELAY_MS);
    if (typeof requestAnimationFrame === "function") {
      requestAnimationFrame(finish);
      return;
    }
    finish();
  });
}
