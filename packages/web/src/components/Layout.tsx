import { A, useLocation } from "@solidjs/router";
import type { ParentProps } from "solid-js";

export function Layout(props: ParentProps) {
  const loc = useLocation();
  return (
    <>
      <header class="top">
        <A href="/" class="brand">Maurice</A>
        <nav class="crumbs" aria-label="Breadcrumb">
          <span>{loc.pathname === "/" ? "workspaces" : ""}</span>
        </nav>
        <span class="spacer" />
        <span class="muted small">alpha</span>
      </header>
      <main class="page">{props.children}</main>
    </>
  );
}
