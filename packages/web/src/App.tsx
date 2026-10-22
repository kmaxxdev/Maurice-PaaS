import { createResource, For, Show } from "solid-js";
import { Panel, Pill, StatusDot } from "@maurice/ui";
import { workspaces } from "./api";

export function App() {
  const [list] = createResource(async () => (await workspaces.list({})).workspaces);
  return (
    <main style={{ "font-family": "var(--m-font)", background: "var(--m-bg)", color: "var(--m-fg)", "min-height": "100vh", padding: "24px" }}>
      <header style={{ display: "flex", "align-items": "baseline", gap: "12px", "margin-bottom": "16px" }}>
        <h1 style={{ margin: 0, "font-size": "20px" }}>Maurice</h1>
        <span style={{ color: "var(--m-muted)" }}>workspaces</span>
      </header>
      <Show when={!list.loading} fallback={<p style={{ color: "var(--m-muted)" }}>Loading…</p>}>
        <Show when={list()?.length} fallback={<Panel>No workspaces yet. Create one with <code>marik workspace create</code>.</Panel>}>
          <div style={{ display: "grid", gap: "12px", "grid-template-columns": "repeat(auto-fill, minmax(260px, 1fr))" }}>
            <For each={list()}>
              {(ws) => (
                <Panel>
                  <div style={{ display: "flex", gap: "8px", "align-items": "center" }}>
                    <StatusDot tone="ok" title="active" />
                    <strong>{ws.name}</strong>
                    <Pill tone={ws.env === 1 ? "warn" : "neutral"}>{ws.env === 1 ? "prod" : "dev"}</Pill>
                  </div>
                  <div style={{ color: "var(--m-muted)", "font-family": "var(--m-mono)", "font-size": "12px", "margin-top": "6px" }}>
                    {ws.unixUser} · uid {ws.uid} · {ws.home}
                  </div>
                </Panel>
              )}
            </For>
          </div>
        </Show>
      </Show>
    </main>
  );
}
