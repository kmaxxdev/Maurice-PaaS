// @maurice/ui: the panel's component kit. Starts tiny and grows with the screens
// that actually need a component. Accessibility primitives come from Kobalte.
import type { JSX, ParentProps } from "solid-js";

export type Tone = "neutral" | "ok" | "warn" | "danger" | "accent";

export function StatusDot(props: { tone: Tone; title?: string }) {
  const color = () =>
    ({ neutral: "var(--m-muted)", ok: "var(--m-ok)", warn: "var(--m-warn)", danger: "var(--m-danger)", accent: "var(--m-accent)" })[props.tone];
  return (
    <span
      role="img"
      aria-label={props.title ?? props.tone}
      title={props.title}
      style={{ display: "inline-block", width: "8px", height: "8px", "border-radius": "50%", background: color() }}
    />
  );
}

export function Pill(props: ParentProps<{ tone?: Tone }>) {
  const tone = () => props.tone ?? "neutral";
  return (
    <span
      style={{
        display: "inline-block",
        "font-size": "12px",
        padding: "1px 8px",
        "border-radius": "999px",
        border: "1px solid var(--m-line)",
        color: tone() === "neutral" ? "var(--m-muted)" : `var(--m-${tone()})`,
      }}
    >
      {props.children}
    </span>
  );
}

export function Button(props: ParentProps<{ onClick?: () => void; danger?: boolean; disabled?: boolean; type?: "button" | "submit" }>) {
  return (
    <button
      type={props.type ?? "button"}
      onClick={props.onClick}
      disabled={props.disabled}
      style={{
        font: "inherit",
        padding: "6px 12px",
        "border-radius": "var(--m-radius)",
        border: `1px solid ${props.danger ? "var(--m-danger)" : "var(--m-line)"}`,
        background: "var(--m-surface)",
        color: props.danger ? "var(--m-danger)" : "var(--m-fg)",
        cursor: props.disabled ? "not-allowed" : "pointer",
      }}
    >
      {props.children}
    </button>
  );
}

export function Panel(props: ParentProps<{ style?: JSX.CSSProperties }>) {
  return (
    <section
      style={{
        background: "var(--m-surface)",
        border: "1px solid var(--m-line)",
        "border-radius": "8px",
        padding: "16px",
        ...props.style,
      }}
    >
      {props.children}
    </section>
  );
}
