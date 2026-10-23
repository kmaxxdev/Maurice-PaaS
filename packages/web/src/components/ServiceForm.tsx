import { For, Show, createSignal } from "solid-js";
import { create } from "@bufbuild/protobuf";
import { ServiceSchema } from "../gen/maurice/v1/services_pb";
import type { Service } from "../gen/maurice/v1/services_pb";
import { ServiceKind } from "../gen/maurice/v1/common_pb";

/**
 * A plain form for the common case. Ports are one per line as
 * `container` or `container:hostPort` or `container:hostIp:hostPort`; env as
 * `KEY=value`, `KEY={{service.field}}` or `KEY=secret:name`.
 */
export function ServiceForm(props: { initial?: Service; secretNames?: string[]; onSubmit: (s: Service) => void; submitLabel?: string }) {
  const init = props.initial;
  const [name, setName] = createSignal(init?.name ?? "");
  const [kind, setKind] = createSignal<"stateless" | "stateful">(init?.kind === ServiceKind.STATEFUL ? "stateful" : "stateless");
  const [sourceKind, setSourceKind] = createSignal<"image" | "build">(init?.source.case === "dockerfilePath" ? "build" : "image");
  const [image, setImage] = createSignal(init?.source.case === "imageRef" ? init.source.value : "");
  const [dockerfile, setDockerfile] = createSignal(init?.source.case === "dockerfilePath" ? init.source.value : "Dockerfile");
  const [ports, setPorts] = createSignal(
    (init?.ports ?? []).map((p) => (p.published ? `${p.containerPort}:${p.hostIp}:${p.hostPort}` : `${p.containerPort}`)).join("\n"),
  );
  const [env, setEnv] = createSignal(
    (init?.env ?? [])
      .map((e) => `${e.key}=${e.value.case === "secretName" ? `secret:${e.value.value}` : e.value.case ? e.value.value : ""}`)
      .join("\n"),
  );
  const [cpu, setCpu] = createSignal(init?.cpu ? String(init.cpu) : "");
  const [memory, setMemory] = createSignal(init?.memory ? String(Number(init.memory) / 1048576) : "");
  const [error, setError] = createSignal("");

  function build(): Service {
    const portList = ports()
      .split("\n")
      .map((l) => l.trim())
      .filter(Boolean)
      .map((l) => {
        const parts = l.split(":");
        const containerPort = Number(parts[0]);
        if (!containerPort) throw new Error(`bad port line: ${l}`);
        if (parts.length === 1) return { containerPort, published: false, hostIp: "", hostPort: 0 };
        if (parts.length === 2) return { containerPort, published: true, hostIp: "127.0.0.1", hostPort: Number(parts[1]) };
        return { containerPort, published: true, hostIp: parts[1], hostPort: Number(parts[2]) };
      });
    const envList = env()
      .split("\n")
      .map((l) => l.trim())
      .filter(Boolean)
      .map((l) => {
        const i = l.indexOf("=");
        if (i <= 0) throw new Error(`bad env line: ${l}`);
        const key = l.slice(0, i);
        const v = l.slice(i + 1);
        if (v.startsWith("secret:")) return { key, value: { case: "secretName" as const, value: v.slice(7) } };
        if (v.startsWith("{{")) return { key, value: { case: "ref" as const, value: v } };
        return { key, value: { case: "literal" as const, value: v } };
      });
    return create(ServiceSchema, {
      name: name().trim(),
      kind: kind() === "stateful" ? ServiceKind.STATEFUL : ServiceKind.STATELESS,
      source: sourceKind() === "image" ? { case: "imageRef", value: image().trim() } : { case: "dockerfilePath", value: dockerfile().trim() },
      ports: portList,
      env: envList,
      cpu: cpu() ? Number(cpu()) : 0,
      memory: memory() ? BigInt(Math.round(Number(memory()) * 1048576)) : 0n,
    });
  }

  return (
    <form
      style={{ display: "grid", gap: "10px" }}
      onSubmit={(e) => {
        e.preventDefault();
        setError("");
        try {
          props.onSubmit(build());
        } catch (err) {
          setError(err instanceof Error ? err.message : String(err));
        }
      }}
    >
      <div class="row">
        <label>
          Name
          <input id="sf-name" value={name()} onInput={(e) => setName(e.currentTarget.value)} required pattern="[a-z0-9]([a-z0-9-]*[a-z0-9])?" placeholder="web" disabled={!!init} />
        </label>
        <label>
          Kind
          <select id="sf-kind" value={kind()} onChange={(e) => setKind(e.currentTarget.value as "stateless" | "stateful")}>
            <option value="stateless">stateless (blue-green later)</option>
            <option value="stateful">stateful (restart, data volume)</option>
          </select>
        </label>
        <label>
          Source
          <select id="sf-source" value={sourceKind()} onChange={(e) => setSourceKind(e.currentTarget.value as "image" | "build")}>
            <option value="image">image</option>
            <option value="build">Dockerfile in the project repository</option>
          </select>
        </label>
        <Show when={sourceKind() === "image"} fallback={
          <label>
            Dockerfile path
            <input id="sf-dockerfile" value={dockerfile()} onInput={(e) => setDockerfile(e.currentTarget.value)} style={{ width: "260px" }} />
          </label>
        }>
          <label>
            Image
            <input id="sf-image" value={image()} onInput={(e) => setImage(e.currentTarget.value)} placeholder="docker.io/library/postgres:16" style={{ width: "300px" }} required />
          </label>
        </Show>
      </div>
      <div class="grid" style={{ "grid-template-columns": "repeat(auto-fit, minmax(260px, 1fr))" }}>
        <label>
          <span>Ports, one per line: <code>5432</code> internal, <code>80:18080</code> published on loopback, <code>80:0.0.0.0:8080</code> public</span>
          <textarea id="sf-ports" class="mono" style={{ "min-height": "80px" }} value={ports()} onInput={(e) => setPorts(e.currentTarget.value)} />
        </label>
        <label>
          <span>Environment, one per line: <code>KEY=value</code>, <code>KEY={"{{"}postgres.url{"}}"}</code>, <code>KEY=secret:name</code></span>
          <textarea id="sf-env" class="mono" style={{ "min-height": "80px" }} value={env()} onInput={(e) => setEnv(e.currentTarget.value)} />
        </label>
      </div>
      <Show when={props.secretNames?.length}>
        <div class="muted small">Secrets in this workspace: <For each={props.secretNames}>{(n) => <code style={{ "margin-right": "6px" }}>{n}</code>}</For></div>
      </Show>
      <div class="row">
        <label>
          CPU cores (empty = unlimited)
          <input id="sf-cpu" value={cpu()} onInput={(e) => setCpu(e.currentTarget.value)} inputmode="decimal" style={{ width: "100px" }} />
        </label>
        <label>
          Memory MiB (empty = unlimited)
          <input id="sf-mem" value={memory()} onInput={(e) => setMemory(e.currentTarget.value)} inputmode="numeric" style={{ width: "120px" }} />
        </label>
        <span style={{ "margin-left": "auto" }} />
        <button class="primary" type="submit">{props.submitLabel ?? "Plan"}</button>
      </div>
      <Show when={error()}>
        <div class="error small">{error()}</div>
      </Show>
    </form>
  );
}
