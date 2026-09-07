import { useLayoutEffect } from "react";

const HEIGHT_PROPERTY = "--mobile-viewport-height";

/** Keep the fixed mobile shell inside iOS's visible area when its keyboard opens. */
export function useMobileVisualViewport(): void {
  useLayoutEffect(() => {
    const root = document.documentElement;
    const viewport = window.visualViewport;

    const update = () => {
      root.style.setProperty(
        HEIGHT_PROPERTY,
        `${Math.round(viewport?.height ?? window.innerHeight)}px`,
      );
      // iOS may pan the layout viewport to reveal xterm's hidden textarea.
      // The app is a fixed shell, so any document scroll is always unwanted.
      window.scrollTo(0, 0);
      root.scrollTop = 0;
      document.body.scrollTop = 0;
    };

    update();
    window.addEventListener("resize", update);
    viewport?.addEventListener("resize", update);
    viewport?.addEventListener("scroll", update);

    return () => {
      window.removeEventListener("resize", update);
      viewport?.removeEventListener("resize", update);
      viewport?.removeEventListener("scroll", update);
      root.style.removeProperty(HEIGHT_PROPERTY);
    };
  }, []);
}
