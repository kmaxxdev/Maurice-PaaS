import { For, Show, createResource, createSignal } from "solid-js";
import { Env } from "../gen/maurice/v1/common_pb";
import type { Plan } from "../gen/maurice/v1/common_pb";
import { TlsMode, Visibility } from "../gen/maurice/v1/edge_pb";
import type { Service } from "../gen/maurice/v1/services_pb";
import { edge, errorText } from "../api";

const tlsLabel: Record<number, string> = { [TlsMode.AUTO]: "https", [TlsMode.INTERNAL]: "https (internal CA)", [TlsMode.OFF]: "http" };

/**
 * Routes of one service on the workspace edge (ADR-009): expose a container
 * port on a block port, optionally with a domain on the host edge.
 */
export function Expose(props: { service: Service; workspaceId: string; onPlan: (p: Plan) => void; refreshKey?: unknown }) {
  const [routes, { refetch }] = createResource(
    () => [props.workspaceId, props.refreshKey] as const,
    async ([ws]) => (await edge.listRoutes({ workspaceId: ws })).routes.filter((r) => r.serviceId === props.service.id),
  );
  const [status] = createResource(() => props.workspaceId, (ws) => edge.getEdgeStatus({ workspaceId: ws }));
  const [port, setPort] = createSignal(props.service.ports[0]?.containerPort ?? 80);
  const [internal, setInternal] = createSignal(false);
  const [domain, setDomain] = createSignal("");
  const [tls, setTls] = createSignal<TlsMode>(TlsMode.AUTO);
  const [newDomain, setNewDomain] = createSignal<Record<string, string>>({});
  const [error, setError] = createSignal("");

  async function run(f: () => Promise<Plan>) {
    setError("");
    try {
      props.onPlan(await f());
    } catch (e) {
      setError(errorText(e));
    }
  }

  const exposePlan = (e: Event) => {
    e.preventDefault();
    run(() =>
      edge.planExpose({
        serviceId: props.service.id,
        containerPort: port(),
        visibility: internal() ? Visibility.INTERNAL : Visibility.PUBLIC,
        domain: internal() ? "" : domain().trim(),
        tls: tls(),
      }),
    );
  };

  return (
    <div class="card" style={{ "grid-column": "1 / -1" }}>
      <div class="row">
        <h3>Expose</h3>
        <span class="muted small">
          workspace edge {status()?.workspaceEdge ?? "…"} · host edge {status()?.hostEdge ?? "…"}
          <Show when={status()?.internalAddr}> · internal address {status()!.internalAddr}</Show>
        </span>
        <span style={{ "margin-left": "auto" }} />
        <button onClick={() => refetch()}>Refresh</button>
      </div>
      <p class="muted small" style={{ margin: "6px 0 10px" }}>
        A route makes a container port reachable on a port of the workspace block through the workspace edge. Public routes get domains on the host edge; internal routes are for other workspaces only. Routes follow redeploys automatically.
      </p>
      <Show when={routes()?.length} fallback={<p class="muted small">Not exposed. Only published ports (if any) reach the host.</p>}>
        <table>
          <thead><tr><th>Edge port</th><th>Container port</th><th>Visibility</th><th>Upstream</th><th>Domains</th><th /></tr></thead>
          <tbody>
            <For each={routes()}>
              {(r) => (
                <tr>
                  <td class="mono">127.0.0.1:{r.blockPort}</td>
                  <td class="mono">{r.containerPort}</td>
                  <td>{r.visibility === Visibility.INTERNAL ? <span class="pill">internal</span> : <span class="pill accent">public</span>}</td>
                  <td class="mono small">{r.upstream || <span class="muted">not deployed</span>}</td>
                  <td>
                    <For each={r.domains}>
                      {(d) => (
                        <div class="row small" style={{ gap: "6px" }}>
                          <code>{d.name}</code>
                          <span class="pill">{tlsLabel[d.tls] ?? "?"}</span>
                          <Show when={d.publicIp}><span class="muted mono">{d.publicIp}</span></Show>
                          <button class="danger" style={{ padding: "1px 6px" }} onClick={() => run(() => edge.planRemoveDomain({ domainId: d.id }))}>remove</button>
                        </div>
                      )}
                    </For>
                    <Show when={r.visibility === Visibility.PUBLIC}>
                      <form class="inline" style={{ "margin-top": "4px" }} onSubmit={(e) => { e.preventDefault(); run(() => edge.planAddDomain({ routeId: r.id, domain: (newDomain()[r.id] ?? "").trim(), tls: tls() })); }}>
                        <input placeholder="app.example.com" value={newDomain()[r.id] ?? ""} onInput={(e) => setNewDomain({ ...newDomain(), [r.id]: e.currentTarget.value })} required style={{ width: "200px" }} />
                        <button type="submit">Add domain</button>
                      </form>
                    </Show>
                  </td>
                  <td><button class="danger" onClick={() => run(() => edge.planUnexpose({ routeId: r.id }))}>Unexpose…</button></td>
                </tr>
              )}
            </For>
          </tbody>
        </table>
      </Show>
      <form class="inline" onSubmit={exposePlan} style={{ "margin-top": "10px" }}>
        <label>
          Container port
          <select id="expose-port" value={String(port())} onChange={(e) => setPort(Number(e.currentTarget.value))}>
            <For each={props.service.ports}>{(p) => <option value={String(p.containerPort)}>{p.containerPort}</option>}</For>
          </select>
        </label>
        <label style={{ "flex-direction": "row", display: "flex", gap: "6px", "align-items": "center" }}>
          <input type="checkbox" id="expose-internal" checked={internal()} onChange={(e) => setInternal(e.currentTarget.checked)} /> internal only
        </label>
        <Show when={!internal()}>
          <label>
            Domain (optional)
            <input id="expose-domain" value={domain()} onInput={(e) => setDomain(e.currentTarget.value)} placeholder="shop.example.com" style={{ width: "220px" }} />
          </label>
          <label>
            TLS
            <select id="expose-tls" value={String(tls())} onChange={(e) => setTls(Number(e.currentTarget.value) as TlsMode)}>
              <option value={String(TlsMode.AUTO)}>https, public CA</option>
              <option value={String(TlsMode.INTERNAL)}>https, internal CA</option>
              <option value={String(TlsMode.OFF)}>http only</option>
            </select>
          </label>
        </Show>
        <button class="primary" type="submit" disabled={!props.service.ports.length}>Expose…</button>
        <Show when={error()}><span class="error small">{error()}</span></Show>
      </form>
      <Show when={props.service.environment === Env.PROD && !routes()?.length}>
        <p class="muted small" style={{ margin: "8px 0 0" }}>Tip: expose prod with a real domain and TLS "public CA"; use "internal CA" on hosts without public DNS.</p>
      </Show>
    </div>
  );
}
