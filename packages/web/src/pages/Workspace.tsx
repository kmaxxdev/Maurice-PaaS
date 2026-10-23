import { A, useParams, useNavigate } from "@solidjs/router";
import { For, Show, createResource, createSignal } from "solid-js";
import { fromJson } from "@bufbuild/protobuf";
import { Env } from "../gen/maurice/v1/common_pb";
import type { Plan } from "../gen/maurice/v1/common_pb";
import { ServiceSchema } from "../gen/maurice/v1/services_pb";
import { ServiceKind } from "../gen/maurice/v1/common_pb";
import type { Service, ServiceStatus } from "../gen/maurice/v1/services_pb";
import { workspaces, services, secrets, errorText } from "../api";
import { PlanDialog } from "../components/PlanDialog";

const exampleService = JSON.stringify(
  {
    name: "postgres",
    kind: "SERVICE_KIND_STATEFUL",
    imageRef: "docker.io/library/postgres:16",
    ports: [{ containerPort: 5432 }],
    env: [{ key: "POSTGRES_PASSWORD", secretName: "pg_admin" }],
  },
  null,
  2,
);

export function stateTone(st?: ServiceStatus) {
  if (!st) return "";
  if (st.activeState === "active") return "ok";
  if (st.activeState === "failed") return "danger";
  if (st.activeState === "activating" || st.activeState === "deactivating") return "warn";
  return "";
}

export function Workspace() {
  const params = useParams();
  const navigate = useNavigate();
  const [ws] = createResource(() => params.id, (id) => workspaces.get({ id }));
  const [list, { refetch }] = createResource(() => params.id, async (id) => (await services.list({ workspaceId: id })).services);
  const [statuses, { refetch: refetchStatus }] = createResource(list, async (svcs) => {
    const out: Record<string, ServiceStatus> = {};
    await Promise.all(svcs.map(async (s) => { try { out[s.id] = await services.getStatus({ serviceId: s.id }); } catch { /* unit may not exist yet */ } }));
    return out;
  });
  const [secretList, { refetch: refetchSecrets }] = createResource(() => params.id, async (id) => (await secrets.list({ workspaceId: id })).secrets);
  const [plan, setPlan] = createSignal<Plan | null>(null);
  const [error, setError] = createSignal("");
  const [json, setJson] = createSignal(exampleService);
  const [project, setProject] = createSignal("default");
  const [secretName, setSecretName] = createSignal("");
  const [secretValue, setSecretValue] = createSignal("");
  const [secretMsg, setSecretMsg] = createSignal("");

  async function planUpsert(e: Event) {
    e.preventDefault();
    setError("");
    try {
      const svc: Service = fromJson(ServiceSchema, JSON.parse(json()));
      setPlan(await services.planUpsert({ workspaceId: params.id, project: project(), service: svc }));
    } catch (err) {
      setError(errorText(err));
    }
  }

  async function setSecret(e: Event) {
    e.preventDefault();
    setSecretMsg("");
    try {
      const info = await secrets.set({ workspaceId: params.id, name: secretName(), value: new TextEncoder().encode(secretValue()) });
      setSecretMsg(`${info.name} stored as version ${info.version}`);
      setSecretName("");
      setSecretValue("");
      refetchSecrets();
    } catch (err) {
      setSecretMsg(errorText(err));
    }
  }

  async function planDelete() {
    setError("");
    try {
      setPlan(await workspaces.planDelete({ id: params.id, purge: true }));
    } catch (err) {
      setError(errorText(err));
    }
  }

  return (
    <>
      <div class="row">
        <A href="/" class="muted">workspaces</A>
        <span class="muted">/</span>
        <h2>{ws()?.name ?? "…"}</h2>
        <Show when={ws()}>{(w) => <span class={`pill ${w().env === Env.PROD ? "warn" : ""}`}>{w().env === Env.PROD ? "prod" : "dev"}</span>}</Show>
        <span class="muted small mono">{ws()?.unixUser} · uid {ws()?.uid}</span>
        <span style={{ "margin-left": "auto" }} />
        <button onClick={() => { refetch(); refetchStatus(); }}>Refresh</button>
        <button class="danger" onClick={planDelete}>Delete workspace</button>
      </div>
      <Show when={error()}>
        <p class="error small">{error()}</p>
      </Show>

      <div class="section">
        <h2>Services</h2>
        <Show when={!list.loading} fallback={<p class="muted">Loading…</p>}>
          <Show when={list()?.length} fallback={<div class="empty">No services. Add one below: the plan shows the unit file before anything is written.</div>}>
            <div class="grid">
              <For each={list()}>
                {(s) => {
                  const st = () => statuses()?.[s.id];
                  return (
                    <div class="card clickable" role="link" tabindex="0" onClick={() => navigate(`/ws/${params.id}/svc/${s.id}`)} onKeyDown={(e) => e.key === "Enter" && navigate(`/ws/${params.id}/svc/${s.id}`)}>
                      <div class="row">
                        <span class={`dot ${stateTone(st())}`} title={st()?.activeState ?? "unknown"} />
                        <h3>{s.name}</h3>
                        <span class="pill">{s.kind === ServiceKind.STATEFUL ? "stateful" : "stateless"}</span>
                        <span class="muted small" style={{ "margin-left": "auto" }}>{st() ? `${st()!.activeState} (${st()!.subState})` : "no unit yet"}</span>
                      </div>
                      <div class="muted small mono" style={{ "margin-top": "6px" }}>
                        {s.source.case === "imageRef" ? s.source.value : s.source.case === "dockerfilePath" ? `build: ${s.source.value}` : "-"}
                      </div>
                      <div class="muted small mono">
                        {s.ports.length ? s.ports.map((p) => (p.published ? `${p.containerPort}→${p.hostIp}:${p.hostPort}` : `${p.containerPort}`)).join("  ") : "no ports"}
                      </div>
                    </div>
                  );
                }}
              </For>
            </div>
          </Show>
        </Show>
      </div>

      <div class="section card">
        <h2 style={{ "margin-bottom": "10px" }}>Add or replace a service</h2>
        <form onSubmit={planUpsert} style={{ display: "grid", gap: "10px" }}>
          <div class="row">
            <label>
              Project
              <input id="svc-project" value={project()} onInput={(e) => setProject(e.currentTarget.value)} pattern="[a-z0-9-]+" />
            </label>
            <span class="muted small">Definition in the API's JSON shape. Secret names must exist in this workspace first.</span>
          </div>
          <textarea id="svc-json" class="mono" value={json()} onInput={(e) => setJson(e.currentTarget.value)} spellcheck={false} />
          <div class="row">
            <button class="primary" type="submit">Plan</button>
          </div>
        </form>
      </div>

      <div class="section card">
        <h2 style={{ "margin-bottom": "10px" }}>Secrets</h2>
        <Show when={secretList()?.length} fallback={<p class="muted small">No secrets. Values are encrypted at rest and only ever referenced by name in units.</p>}>
          <table>
            <thead><tr><th>Name</th><th>Version</th><th>Id</th></tr></thead>
            <tbody>
              <For each={secretList()}>{(s) => <tr><td><code>{s.name}</code></td><td>v{s.version}</td><td class="muted mono small">{s.id}</td></tr>}</For>
            </tbody>
          </table>
        </Show>
        <form class="inline" onSubmit={setSecret} style={{ "margin-top": "10px" }}>
          <label>
            Name
            <input id="secret-name" value={secretName()} onInput={(e) => setSecretName(e.currentTarget.value)} placeholder="pg_admin" required pattern="[A-Za-z0-9_.-]+" />
          </label>
          <label>
            Value
            <input id="secret-value" type="password" value={secretValue()} onInput={(e) => setSecretValue(e.currentTarget.value)} required autocomplete="off" />
          </label>
          <button type="submit">Set</button>
          <Show when={secretMsg()}>
            <span class="small">{secretMsg()}</span>
          </Show>
        </form>
      </div>

      <PlanDialog
        plan={plan()}
        onClose={() => setPlan(null)}
        onApplied={() => {
          if (plan()?.changes.some((c) => c.kind === 2 /* DELETE_USER */)) navigate("/");
          else { refetch(); refetchStatus(); }
        }}
      />
    </>
  );
}
