import { For, Show } from "solid-js";

/** Tiny inline chart for a rolling series; scaled to its own max (or a given one). */
export function Sparkline(props: { values: number[]; max?: number; width?: number; height?: number; unit?: string; label: string }) {
  const w = () => props.width ?? 220;
  const h = () => props.height ?? 44;
  const max = () => Math.max(props.max ?? 0, ...props.values, 1e-9);
  const pts = () => {
    const n = props.values.length;
    if (n === 0) return "";
    return props.values.map((v, i) => `${(i / Math.max(n - 1, 1)) * w()},${h() - (v / max()) * (h() - 2) - 1}`).join(" ");
  };
  const last = () => props.values[props.values.length - 1];
  return (
    <div style={{ display: "grid", gap: "2px" }}>
      <div class="row small" style={{ "justify-content": "space-between" }}>
        <span class="muted">{props.label}</span>
        <Show when={last() !== undefined}>
          <strong style={{ "font-variant-numeric": "tabular-nums" }}>
            {last()!.toFixed(last()! < 10 ? 2 : 0)}{props.unit ?? ""}
          </strong>
        </Show>
      </div>
      <svg viewBox={`0 0 ${w()} ${h()}`} width="100%" height={h()} preserveAspectRatio="none" role="img" aria-label={`${props.label} over the last ${props.values.length} samples`}>
        <For each={[0.25, 0.5, 0.75]}>{(f) => <line x1="0" x2={w()} y1={h() * f} y2={h() * f} stroke="var(--m-line)" stroke-width="1" />}</For>
        <Show when={props.values.length > 1}>
          <polygon points={`0,${h()} ${pts()} ${w()},${h()}`} fill="var(--m-accent-soft)" />
          <polyline points={pts()} fill="none" stroke="var(--m-accent)" stroke-width="1.5" vector-effect="non-scaling-stroke" />
        </Show>
      </svg>
    </div>
  );
}

/** Horizontal usage bar with a label, e.g. memory against its limit. */
export function Meter(props: { value: number; max: number; label: string; format: (v: number) => string }) {
  const pct = () => (props.max > 0 ? Math.min(100, (props.value / props.max) * 100) : 0);
  return (
    <div style={{ display: "grid", gap: "4px" }}>
      <div class="row small" style={{ "justify-content": "space-between" }}>
        <span class="muted">{props.label}</span>
        <span style={{ "font-variant-numeric": "tabular-nums" }}>{props.format(props.value)}{props.max > 0 ? ` / ${props.format(props.max)}` : ""}</span>
      </div>
      <div style={{ height: "8px", "border-radius": "4px", background: "var(--m-surface-2)", border: "1px solid var(--m-line)", overflow: "hidden" }} role="meter" aria-valuenow={pct()} aria-valuemin={0} aria-valuemax={100} aria-label={props.label}>
        <div style={{ width: `${pct()}%`, height: "100%", background: pct() > 90 ? "var(--m-danger)" : pct() > 70 ? "var(--m-warn)" : "var(--m-accent)", transition: "width .3s" }} />
      </div>
    </div>
  );
}
