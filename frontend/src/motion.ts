export function prefersReducedMotion(): boolean {
  return (
    typeof window !== "undefined" &&
    window.matchMedia("(prefers-reduced-motion: reduce)").matches
  );
}

export function scrollIntoViewWithMotion(
  element: HTMLElement,
  options: ScrollIntoViewOptions = {},
) {
  element.scrollIntoView({
    ...options,
    behavior: prefersReducedMotion() ? "auto" : "smooth",
  });
}
