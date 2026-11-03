import { A, useParams, useNavigate } from "@solidjs/router";
import { For, Show, createResource, createSignal } from "solid-js";
import { fromJson } from "@bufbuild/protobuf";
import { Env } from "../gen/maurice/v1/common_pb";
import type { Plan } from "../gen/maurice/v1/common_pb";
import { ServiceSchema } from "../gen/maurice/v1/services_pb";
import { ServiceKind } from "../gen/maurice/v1/common_pb";
import type { Service, ServiceStatus } from "../gen/maurice/v1/services_pb";
import { workspaces, services, secrets, projects, errorText } from "../api";
import { PlanDialog } from "../components/PlanDialog";
import { ServiceForm } from "../components/ServiceForm";
import { ENV_ORDER, envLabel, envTone } from "../env";

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
  const [advanced, setAdvanced] = createSignal(false);
  const [project, setProject] = createSignal("default");
  const [targetEnv, setTargetEnv] = createSignal<Env>(Env.UNSPECIFIED);
  // The environment new services go to: the chosen one, else the first the workspace has.
  const envForNew = () => (targetEnv() !== Env.UNSPECIFIED ? targetEnv() : ws()?.environments[0] ?? Env.UNSPECIFIED);
  const missingEnvs = () => ENV_ORDER.filter((e) => !(ws()?.environments ?? []).includes(e));
  const [secretName, setSecretName] = createSignal("");
  const [secretValue, setSecretValue] = createSignal("");
  const [secretEnv, setSecretEnv] = createSignal<Env>(Env.UNSPECIFIED);
  const [secretMsg, setSecretMsg] = createSignal("");
  const [projectList, { refetch: refetchProjects }] = createResource(() => params.id, async (id) => (await projects.list({ workspaceId: id })).projects);
  const [prName, setPrName] = createSignal("default");
  const [prRepo, setPrRepo] = createSignal("");
  const [prRef, setPrRef] = createSignal("main");
  const [prKey, setPrKey] = createSignal("");
  const [prMsg, setPrMsg] = createSignal("");

  async function saveProject(e: Event) {
    e.preventDefault();
    setPrMsg("");
    try {
      const p = await projects.upsert({ workspaceId: params.id, name: prName(), gitRepo: prRepo(), gitRef: prRef(), gitKeySecret: prKey() });
      setPrMsg(`${p.name} → ${p.gitRepo || "(no repository)"} @ ${p.gitRef}`);
      refetchProjects();
    } catch (err) {
      setPrMsg(errorText(err));
    }
  }

  async function planUpsertJson(e: Event) {
    e.preventDefault();
    setError("");
    try {
      const svc: Service = fromJson(ServiceSchema, JSON.parse(json()));
      await planUpsert(svc);
    } catch (err) {
      setError(errorText(err));
    }
  }

  async function planUpsert(svc: Service) {
    setError("");
    try {
      setPlan(await services.planUpsert({ workspaceId: params.id, project: project(), service: svc, environment: envForNew() }));
    } catch (err) {
      setError(errorText(err));
    }
  }

  async function planAddEnv(e: Env) {
    setError("");
    try {
      setPlan(await workspaces.planAddEnvironment({ workspaceId: params.id, environment: e }));
    } catch (err) {
      setError(errorText(err));
    }
  }

  async function setSecret(e: Event) {
    e.preventDefault();
    setSecretMsg("");
    try {
      const info = await secrets.set({ workspaceId: params.id, name: secretName(), value: new TextEncoder().encode(secretValue()), environment: secretEnv() });
      setSecretMsg(`${info.name} (${envLabel(info.environment)}) stored as version ${info.version}`);
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
        <For each={ws()?.environments ?? []}>{(e) => <span class={`pill ${envTone(e)}`}>{envLabel(e)}</span>}</For>
        <span class="muted small mono">{ws()?.unixUser} · uid {ws()?.uid}</span>
        <span style={{ "margin-left": "auto" }} />
        <For each={missingEnvs()}>{(e) => <button onClick={() => planAddEnv(e)}>Add {envLabel(e)}</button>}</For>
        <button onClick={() => { refetch(); refetchStatus(); }}>Refresh</button>
        <button class="danger" onClick={planDelete}>Delete workspace</button>
      </div>
      <Show when={error()}>
        <p class="error small">{error()}</p>
      </Show>

      <Show when={!list.loading} fallback={<p class="muted">Loading…</p>}>
      <For each={ws()?.environments ?? []}>
        {(env) => (
      <div class="section">
        <div class="row">
          <h2>Services</h2>
          <span class={`pill ${envTone(env)}`}>{envLabel(env)}</span>
          <span class="muted small mono">network {ws()?.name}-{envLabel(env)}</span>
        </div>
          <Show when={list()?.some((s) => s.environment === env)} fallback={<div class="empty">No services in {envLabel(env)}. Add one below: the plan shows the unit file before anything is written.</div>}>
            <div class="grid">
              <For each={list()?.filter((s) => s.environment === env)}>
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
      </div>
        )}
      </For>
      </Show>

      <div class="section card">
        <div class="row" style={{ "margin-bottom": "10px" }}>
          <h2>Add or replace a service</h2>
          <label style={{ "flex-direction": "row", display: "flex", gap: "6px", "align-items": "center" }}>
            Environment
            <select id="svc-env" value={String(envForNew())} onChange={(e) => setTargetEnv(Number(e.currentTarget.value) as Env)}>
              <For each={ws()?.environments ?? []}>{(e) => <option value={String(e)}>{envLabel(e)}</option>}</For>
            </select>
          </label>
          <label style={{ "flex-direction": "row", display: "flex", gap: "6px", "align-items": "center" }}>
            Project
            <input id="svc-project" value={project()} onInput={(e) => setProject(e.currentTarget.value)} pattern="[a-z0-9-]+" style={{ width: "120px" }} />
          </label>
          <span style={{ "margin-left": "auto" }} />
          <button type="button" onClick={() => setAdvanced(!advanced())}>{advanced() ? "Simple form" : "JSON"}</button>
        </div>
        <Show when={advanced()} fallback={<ServiceForm secretNames={secretList()?.filter((s) => s.environment === Env.UNSPECIFIED || s.environment === envForNew()).map((s) => s.name)} onSubmit={planUpsert} />}>
          <form onSubmit={planUpsertJson} style={{ display: "grid", gap: "10px" }}>
            <span class="muted small">Definition in the API's JSON shape. Secret names must exist in the target environment (or be shared) first.</span>
            <textarea id="svc-json" class="mono" value={json()} onInput={(e) => setJson(e.currentTarget.value)} spellcheck={false} />
            <div class="row">
              <button class="primary" type="submit">Plan</button>
            </div>
          </form>
        </Show>
      </div>

      <div class="section card">
        <h2 style={{ "margin-bottom": "10px" }}>Projects</h2>
        <p class="muted small" style={{ margin: "0 0 8px" }}>A project groups services from one repository. Services built from a Dockerfile need the repository and branch set here.</p>
        <Show when={projectList()?.length}>
          <table>
            <thead><tr><th>Name</th><th>Repository</th><th>Ref</th><th>Deploy key</th></tr></thead>
            <tbody>
              <For each={projectList()}>{(p) => <tr><td>{p.name}</td><td class="mono small">{p.gitRepo || <span class="muted">none</span>}</td><td class="mono">{p.gitRef}</td><td class="mono">{p.gitKeySecret || <span class="muted">none</span>}</td></tr>}</For>
            </tbody>
          </table>
        </Show>
        <form class="inline" onSubmit={saveProject} style={{ "margin-top": "10px" }}>
          <label>
            Project
            <input id="pr-name" value={prName()} onInput={(e) => setPrName(e.currentTarget.value)} required pattern="[a-z0-9-]+" />
          </label>
          <label>
            Repository URL
            <input id="pr-repo" value={prRepo()} onInput={(e) => setPrRepo(e.currentTarget.value)} placeholder="https://github.com/org/app.git" style={{ width: "320px" }} />
          </label>
          <label>
            Ref
            <input id="pr-ref" value={prRef()} onInput={(e) => setPrRef(e.currentTarget.value)} />
          </label>
          <label>
            Deploy key secret
            <input id="pr-key" value={prKey()} onInput={(e) => setPrKey(e.currentTarget.value)} placeholder="optional" />
          </label>
          <button type="submit">Save</button>
          <Show when={prMsg()}>
            <span class="small">{prMsg()}</span>
          </Show>
        </form>
      </div>

      <div class="section card">
        <h2 style={{ "margin-bottom": "10px" }}>Secrets</h2>
        <p class="muted small" style={{ margin: "0 0 8px" }}>Shared secrets are visible to every environment (deploy keys live here); environment secrets override them by name, so staging can hold real provider keys that dev never sees.</p>
        <Show when={secretList()?.length} fallback={<p class="muted small">No secrets. Values are encrypted at rest and only ever referenced by name in units.</p>}>
          <table>
            <thead><tr><th>Scope</th><th>Name</th><th>Version</th><th>Id</th></tr></thead>
            <tbody>
              <For each={secretList()}>{(s) => <tr><td><span class={`pill ${envTone(s.environment)}`}>{envLabel(s.environment)}</span></td><td><code>{s.name}</code></td><td>v{s.version}</td><td class="muted mono small">{s.id}</td></tr>}</For>
            </tbody>
          </table>
        </Show>
        <form class="inline" onSubmit={setSecret} style={{ "margin-top": "10px" }}>
          <label>
            Scope
            <select id="secret-env" value={String(secretEnv())} onChange={(e) => setSecretEnv(Number(e.currentTarget.value) as Env)}>
              <option value={String(Env.UNSPECIFIED)}>shared</option>
              <For each={ws()?.environments ?? []}>{(e) => <option value={String(e)}>{envLabel(e)}</option>}</For>
            </select>
          </label>
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
