import { A } from "@solidjs/router";
import { For, Show, createResource, createSignal } from "solid-js";
import { Env } from "../gen/maurice/v1/common_pb";
import type { Plan } from "../gen/maurice/v1/common_pb";
import { workspaces, errorText } from "../api";
import { PlanDialog } from "../components/PlanDialog";
import { ENV_ORDER, envLabel, envTone } from "../env";

export function Home() {
  const [list, { refetch }] = createResource(async () => (await workspaces.list({})).workspaces);
  const [name, setName] = createSignal("");
  const [envs, setEnvs] = createSignal<Env[]>([Env.DEV, Env.PROD]);
  const toggleEnv = (e: Env, on: boolean) => setEnvs((cur) => (on ? ENV_ORDER.filter((x) => x === e || cur.includes(x)) : cur.filter((x) => x !== e)));
  const [plan, setPlan] = createSignal<Plan | null>(null);
  const [error, setError] = createSignal("");

  async function planCreate(e: Event) {
    e.preventDefault();
    setError("");
    try {
      if (!envs().length) throw new Error("pick at least one environment");
      setPlan(await workspaces.planCreate({ name: name(), environments: envs() }));
    } catch (err) {
      setError(errorText(err));
    }
  }

  return (
    <>
      <div class="row">
        <h2>Workspaces</h2>
        <span class="muted small">one system user each; dev, staging and prod environments inside, each with its own network</span>
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
                      <For each={ws.environments}>{(e) => <span class={`pill ${envTone(e)}`}>{envLabel(e)}</span>}</For>
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
          <fieldset style={{ display: "flex", gap: "12px", border: "none", padding: "0", margin: "0", "align-items": "end" }}>
            <legend class="small">Environments</legend>
            <For each={ENV_ORDER}>
              {(e) => (
                <label style={{ "flex-direction": "row", display: "flex", gap: "6px", "align-items": "center" }}>
                  <input type="checkbox" id={`ws-env-${envLabel(e)}`} checked={envs().includes(e)} onChange={(ev) => toggleEnv(e, ev.currentTarget.checked)} />
                  {envLabel(e)}
                </label>
              )}
            </For>
          </fieldset>
          <button class="primary" type="submit">Plan</button>
          <Show when={error()}>
            <span class="error small">{error()}</span>
          </Show>
        </form>
        <p class="muted small" style={{ margin: "8px 0 0" }}>
          Creates the system user <code>ws-&lt;name&gt;</code> with its own subuid range and home, and one network per environment. Environments can be added later. Nothing happens until you apply the plan.
        </p>
      </div>
      <PlanDialog plan={plan()} onClose={() => setPlan(null)} onApplied={() => { setName(""); refetch(); }} />
    </>
  );
}
