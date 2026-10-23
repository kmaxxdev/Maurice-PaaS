import { A } from "@solidjs/router";
import { For, Show, createResource, createSignal } from "solid-js";
import { Env } from "../gen/maurice/v1/common_pb";
import type { Plan } from "../gen/maurice/v1/common_pb";
import { workspaces, errorText } from "../api";
import { PlanDialog } from "../components/PlanDialog";

export function Home() {
  const [list, { refetch }] = createResource(async () => (await workspaces.list({})).workspaces);
  const [name, setName] = createSignal("");
  const [env, setEnv] = createSignal<"dev" | "prod">("dev");
  const [plan, setPlan] = createSignal<Plan | null>(null);
  const [error, setError] = createSignal("");

  async function planCreate(e: Event) {
    e.preventDefault();
    setError("");
    try {
      setPlan(await workspaces.planCreate({ name: name(), env: env() === "prod" ? Env.PROD : Env.DEV }));
    } catch (err) {
      setError(errorText(err));
    }
  }

  return (
    <>
      <div class="row">
        <h2>Workspaces</h2>
        <span class="muted small">one system user, one network, one set of services each</span>
      </div>
      <div class="section">
        <Show when={!list.loading} fallback={<p class="muted">Loading…</p>}>
          <Show when={list()?.length} fallback={<div class="empty">No workspaces yet. Create the first one below.</div>}>
            <div class="grid">
              <For each={list()}>
                {(ws) => (
                  <A href={`/ws/${ws.id}`} class="card clickable" style={{ display: "block", color: "inherit" }}>
                    <div class="row">
                      <h3>{ws.name}</h3>
                      <span class={`pill ${ws.env === Env.PROD ? "warn" : ""}`}>{ws.env === Env.PROD ? "prod" : "dev"}</span>
                    </div>
                    <div class="muted small mono" style={{ "margin-top": "6px" }}>
                      {ws.unixUser} · uid {ws.uid} · subuid {ws.subuidStart}+{ws.subuidLen}
                    </div>
                    <div class="muted small mono">{ws.home}</div>
                  </A>
                )}
              </For>
            </div>
          </Show>
        </Show>
      </div>
      <div class="section card">
        <h2 style={{ "margin-bottom": "10px" }}>New workspace</h2>
        <form class="inline" onSubmit={planCreate}>
          <label>
            Name
            <input id="ws-name" value={name()} onInput={(e) => setName(e.currentTarget.value)} placeholder="shop" required pattern="[a-z0-9-]{1,28}" />
          </label>
          <label>
            Environment
            <select id="ws-env" value={env()} onChange={(e) => setEnv(e.currentTarget.value as "dev" | "prod")}>
              <option value="dev">dev</option>
              <option value="prod">prod</option>
            </select>
          </label>
          <button class="primary" type="submit">Plan</button>
          <Show when={error()}>
            <span class="error small">{error()}</span>
          </Show>
        </form>
        <p class="muted small" style={{ margin: "8px 0 0" }}>
          Creates the system user <code>ws-&lt;name&gt;</code> with its own subuid range and home. Nothing happens until you apply the plan.
        </p>
      </div>
      <PlanDialog plan={plan()} onClose={() => setPlan(null)} onApplied={() => { setName(""); refetch(); }} />
    </>
  );
}
