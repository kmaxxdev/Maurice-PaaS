import { A, useParams, useNavigate } from "@solidjs/router";
import { Tabs } from "@kobalte/core/tabs";
import { For, Show, createResource, createSignal } from "solid-js";
import { fromJson, toJson } from "@bufbuild/protobuf";
import type { Plan } from "../gen/maurice/v1/common_pb";
import { ServiceSchema } from "../gen/maurice/v1/services_pb";
import { ServiceKind } from "../gen/maurice/v1/common_pb";
import type { LogLine, Service, StatsSample } from "../gen/maurice/v1/services_pb";
import { workspaces, services, errorText } from "../api";
import { PlanDialog } from "../components/PlanDialog";
import { ServiceForm } from "../components/ServiceForm";
import { Sparkline, Meter } from "../components/Sparkline";
import { onCleanup, createEffect } from "solid-js";
import { stateTone } from "./Workspace";

export function ServiceView() {
  const params = useParams();
  const navigate = useNavigate();
  const [ws] = createResource(() => params.wsId, (id) => workspaces.get({ id }));
  const [svc, { refetch: refetchSvc }] = createResource(() => params.id, (id) => services.get({ id }));
  const [status, { refetch: refetchStatus }] = createResource(() => params.id, async (id) => { try { return await services.getStatus({ serviceId: id }); } catch { return undefined; } });
  const [unit, { refetch: refetchUnit }] = createResource(() => params.id, (id) => services.getUnit({ serviceId: id }));
  const [tail, setTail] = createSignal(200);
  const [follow, setFollow] = createSignal(false);
  const [liveLines, setLiveLines] = createSignal<LogLine[]>([]);
  const [logs, { refetch: refetchLogs }] = createResource(
    () => [params.id, tail()] as const,
    async ([id, n]) => {
      const out: LogLine[] = [];
      for await (const l of services.logs({ serviceId: id, tail: n })) out.push(l);
      return out;
    },
  );
  // Follow mode: a long-lived stream appended to the tail; aborted when switched off or on leave.
  let followAbort: AbortController | undefined;
  createEffect(() => {
    followAbort?.abort();
    followAbort = undefined;
    setLiveLines([]);
    if (!follow()) return;
    const ac = new AbortController();
    followAbort = ac;
    (async () => {
      try {
        let skip = tail();
        for await (const l of services.logs({ serviceId: params.id, tail: tail(), follow: true }, { signal: ac.signal })) {
          if (skip > 0) { skip--; continue; }
          setLiveLines((prev) => [...prev.slice(-1000), l]);
        }
      } catch { /* aborted */ }
    })();
  });
  onCleanup(() => followAbort?.abort());
  // Live stats while the overview is open.
  const [stats, setStats] = createSignal<StatsSample | null>(null);
  const [cpuHist, setCpuHist] = createSignal<number[]>([]);
  const [memHist, setMemHist] = createSignal<number[]>([]);
  const [netHist, setNetHist] = createSignal<number[]>([]);
  let lastNet: { t: number; rx: number; tx: number } | null = null;
  let statsAbort: AbortController | undefined;
  createEffect(() => {
    const id = params.id;
    statsAbort?.abort();
    const ac = new AbortController();
    statsAbort = ac;
    (async () => {
      try {
        for await (const s of services.stats({ serviceId: id }, { signal: ac.signal })) {
          setStats(s);
          const running = s.memoryBytes > 0n;
          setCpuHist((h) => [...h.slice(-59), running ? s.cpuCores : 0]);
          setMemHist((h) => [...h.slice(-59), running ? Number(s.memoryBytes) / 1048576 : 0]);
          const now = Number(s.time?.seconds ?? 0n);
          const rx = Number(s.netRxBytes), tx = Number(s.netTxBytes);
          if (lastNet && now > lastNet.t && running) {
            const kbps = ((rx - lastNet.rx + tx - lastNet.tx) / (now - lastNet.t)) / 1024;
            setNetHist((h) => [...h.slice(-59), Math.max(0, kbps)]);
          }
          lastNet = running ? { t: now, rx, tx } : null;
        }
      } catch { /* aborted */ }
    })();
  });
  onCleanup(() => statsAbort?.abort());
  const [plan, setPlan] = createSignal<Plan | null>(null);
  const [error, setError] = createSignal("");
  const [busy, setBusy] = createSignal("");
  const [json, setJson] = createSignal<string | null>(null);

  const definitionJson = () => json() ?? (svc() ? JSON.stringify(toJson(ServiceSchema, svc()!), null, 2) : "");

  function refreshAll() {
    refetchSvc();
    refetchStatus();
    refetchUnit();
    refetchLogs();
  }

  async function run(label: string, f: () => Promise<unknown>) {
    setError("");
    setBusy(label);
    try {
      await f();
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy("");
      refreshAll();
    }
  }

  async function planUpsert(e: Event) {
    e.preventDefault();
    setError("");
    try {
      const def: Service = fromJson(ServiceSchema, JSON.parse(definitionJson()));
      setPlan(await services.planUpsert({ workspaceId: params.wsId, project: "", service: def }));
    } catch (err) {
      setError(errorText(err));
    }
  }

  const fmtTime = (l: LogLine) => {
    const s = Number(l.time?.seconds ?? 0n);
    return s ? new Date(s * 1000).toISOString().slice(11, 19) : "";
  };

  return (
    <>
      <div class="row">
        <A href="/" class="muted">workspaces</A>
        <span class="muted">/</span>
        <A href={`/ws/${params.wsId}`} class="muted">{ws()?.name ?? "…"}</A>
        <span class="muted">/</span>
        <span class={`dot ${stateTone(status())}`} />
        <h2>{svc()?.name ?? "…"}</h2>
        <Show when={svc()}>{(s) => <span class="pill">{s().kind === ServiceKind.STATEFUL ? "stateful" : "stateless"}</span>}</Show>
        <span class="muted small">{status() ? `${status()!.activeState} (${status()!.subState})` : "no unit yet"}</span>
        <span style={{ "margin-left": "auto" }} />
        <button class="primary" disabled={!!busy()} onClick={() => run("deploy", async () => setPlan(await services.planDeploy({ serviceId: params.id })))}>Deploy…</button>
        <button disabled={!!busy() || status()?.activeState !== "active"} onClick={() => run("restart", () => services.restart({ serviceId: params.id }))}>{busy() === "restart" ? "Restarting…" : "Restart"}</button>
        <button disabled={!!busy() || status()?.activeState !== "active"} onClick={() => run("stop", () => services.stop({ serviceId: params.id }))}>{busy() === "stop" ? "Stopping…" : "Stop"}</button>
        <button class="danger" disabled={!!busy()} onClick={() => run("delete", async () => setPlan(await services.planDelete({ id: params.id, purgeVolumes: true })))}>Delete…</button>
      </div>
      <Show when={error()}>
        <p class="error small">{error()}</p>
      </Show>

      <Tabs class="section" defaultValue="overview">
        <Tabs.List class="tabs-list">
          <Tabs.Trigger value="overview">Overview</Tabs.Trigger>
          <Tabs.Trigger value="unit">Unit</Tabs.Trigger>
          <Tabs.Trigger value="logs">Logs</Tabs.Trigger>
          <Tabs.Trigger value="definition">Definition</Tabs.Trigger>
          <Tabs.Indicator class="tabs-indicator" />
        </Tabs.List>

        <Tabs.Content value="overview">
          <Show when={svc()}>
            {(s) => (
              <div class="grid">
                <div class="card">
                  <h3>Runtime</h3>
                  <table>
                    <tbody>
                      <tr><th>State</th><td>{status() ? `${status()!.activeState} (${status()!.subState})` : "unknown"}</td></tr>
                      <tr><th>Image</th><td class="mono">{status()?.image || (s().source.case === "imageRef" ? s().source.value : "-")}</td></tr>
                      <tr><th>Deployment</th><td class="mono small">{status()?.deploymentId || "none yet"}</td></tr>
                      <tr><th>Since</th><td>{status()?.since ? new Date(Number(status()!.since!.seconds) * 1000).toLocaleString() : "-"}</td></tr>
                    </tbody>
                  </table>
                </div>
                <div class="card" style={{ display: "grid", gap: "12px", "align-content": "start" }}>
                  <h3>Usage <span class="muted small" style={{ "font-weight": "400" }}>live, 2 s samples</span></h3>
                  <Show when={stats()} fallback={<p class="muted small">Waiting for the first sample…</p>}>
                    <Show when={stats()!.memoryBytes > 0n || cpuHist().some((v) => v > 0)} fallback={<p class="muted small">Not running.</p>}>
                      <Sparkline label="CPU" values={cpuHist()} unit=" cores" max={s().cpu || undefined} />
                      <Sparkline label="Memory" values={memHist()} unit=" MiB" max={s().memory ? Number(s().memory) / 1048576 : undefined} />
                      <Sparkline label="Network" values={netHist()} unit=" KiB/s" />
                      <Show when={s().memory}>
                        <Meter label="Memory limit" value={Number(stats()!.memoryBytes) / 1048576} max={Number(s().memory) / 1048576} format={(v) => `${v.toFixed(0)} MiB`} />
                      </Show>
                      <Show when={s().cpu}>
                        <Meter label="CPU quota" value={stats()!.cpuCores} max={s().cpu} format={(v) => `${v.toFixed(2)} c`} />
                      </Show>
                    </Show>
                  </Show>
                </div>
                <div class="card">
                  <h3>Ports</h3>
                  <Show when={s().ports.length} fallback={<p class="muted small">No ports declared.</p>}>
                    <table>
                      <thead><tr><th>Container</th><th>Published</th></tr></thead>
                      <tbody>
                        <For each={s().ports}>
                          {(p) => (
                            <tr>
                              <td class="mono">{p.containerPort}</td>
                              <td class="mono">{p.published ? `${p.hostIp}:${p.hostPort}` : <span class="muted">internal only</span>}</td>
                            </tr>
                          )}
                        </For>
                      </tbody>
                    </table>
                  </Show>
                </div>
                <div class="card">
                  <h3>Environment</h3>
                  <Show when={s().env.length} fallback={<p class="muted small">No variables.</p>}>
                    <table>
                      <tbody>
                        <For each={s().env}>
                          {(e) => (
                            <tr>
                              <th class="mono">{e.key}</th>
                              <td class="mono">
                                {e.value.case === "literal" ? e.value.value : e.value.case === "ref" ? <span class="pill accent">{e.value.value}</span> : e.value.case === "secretName" ? <span class="pill">secret {e.value.value}</span> : "-"}
                              </td>
                            </tr>
                          )}
                        </For>
                      </tbody>
                    </table>
                  </Show>
                </div>
                <div class="card">
                  <h3>Resources</h3>
                  <table>
                    <tbody>
                      <tr><th>CPU</th><td>{s().cpu ? `${s().cpu} cores` : "unlimited"}</td></tr>
                      <tr><th>Memory</th><td>{s().memory ? `${(Number(s().memory) / 1048576).toFixed(0)} MiB` : "unlimited"}</td></tr>
                      <tr><th>Isolated user</th><td>{s().isolated ? "yes" : "no"}</td></tr>
                    </tbody>
                  </table>
                </div>
              </div>
            )}
          </Show>
        </Tabs.Content>

        <Tabs.Content value="unit">
          <p class="muted small mono">{unit()?.path}</p>
          <Show when={unit()?.content} fallback={<div class="empty">No unit on disk yet. Apply an upsert plan first.</div>}>
            <pre>{unit()!.content}</pre>
          </Show>
        </Tabs.Content>

        <Tabs.Content value="logs">
          <div class="row" style={{ "margin-bottom": "10px" }}>
            <label>
              Tail
              <select id="log-tail" value={String(tail())} onChange={(e) => setTail(Number(e.currentTarget.value))}>
                <option value="100">100</option>
                <option value="200">200</option>
                <option value="1000">1000</option>
              </select>
            </label>
            <button onClick={() => refetchLogs()}>Refresh</button>
            <label style={{ "flex-direction": "row", display: "flex", gap: "6px", "align-items": "center" }}>
              <input type="checkbox" id="log-follow" checked={follow()} onChange={(e) => setFollow(e.currentTarget.checked)} /> follow
            </label>
          </div>
          <Show when={!logs.loading} fallback={<p class="muted">Loading…</p>}>
            <Show when={logs()?.length} fallback={<div class="empty">No journal lines for this unit.</div>}>
              <pre class="log">
                <For each={[...(logs() ?? []), ...liveLines()]}>
                  {(l) => (
                    <div class={`p${l.priority}`}>
                      <span class="t">{fmtTime(l)}</span> {l.message}
                    </div>
                  )}
                </For>
              </pre>
            </Show>
          </Show>
        </Tabs.Content>

        <Tabs.Content value="definition">
          <p class="muted small" style={{ margin: "0 0 10px" }}>Edit and plan: the plan shows the unit diff, nothing changes until it is applied.</p>
          <Show when={svc()}>
            {(s) => (
              <ServiceForm
                initial={s()}
                onSubmit={async (def) => {
                  setError("");
                  try {
                    setPlan(await services.planUpsert({ workspaceId: params.wsId, project: "", service: def }));
                  } catch (err) {
                    setError(errorText(err));
                  }
                }}
              />
            )}
          </Show>
          <details style={{ "margin-top": "14px" }}>
            <summary class="muted small">JSON</summary>
          <form onSubmit={planUpsert} style={{ display: "grid", gap: "10px", "margin-top": "8px" }}>
            <textarea id="def-json" class="mono" value={definitionJson()} onInput={(e) => setJson(e.currentTarget.value)} spellcheck={false} />
            <div class="row">
              <button class="primary" type="submit">Plan</button>
              <button type="button" onClick={() => setJson(null)}>Reset</button>
            </div>
          </form>
          </details>
        </Tabs.Content>
      </Tabs>

      <PlanDialog
        plan={plan()}
        onClose={() => setPlan(null)}
        onApplied={() => {
          if (plan()?.changes.some((c) => c.kind === 4 /* REMOVE_UNIT */)) navigate(`/ws/${params.wsId}`);
          else refreshAll();
        }}
      />
    </>
  );
}
