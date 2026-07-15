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
  const behavior: ScrollBehavior = prefersReducedMotion() ? "auto" : "smooth";
  if (options.block === "start") {
    const content = element.closest<HTMLElement>(".content");
    const offsetValue = getComputedStyle(document.documentElement)
      .getPropertyValue("--console-sticky-offset")
      .trim();
    const parsedOffset = Number.parseFloat(offsetValue);
    const offset = Number.isFinite(parsedOffset) ? parsedOffset : 0;
    const contentOverflow = content
      ? getComputedStyle(content).overflowY
      : "visible";
    const contentScrolls =
      content !== null && /^(auto|scroll|overlay)$/.test(contentOverflow);
    if (contentScrolls) {
      const contentTop = content.getBoundingClientRect().top;
      const elementTop = element.getBoundingClientRect().top;
      content.scrollTo({
        behavior,
        top: Math.max(0, content.scrollTop + elementTop - contentTop - offset),
      });
      return;
    }
    window.scrollTo({
      behavior,
      top: Math.max(0, window.scrollY + element.getBoundingClientRect().top - offset),
    });
    return;
  }
  element.scrollIntoView({
    ...options,
    behavior,
  });
}
