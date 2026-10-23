import { Dialog } from "@kobalte/core/dialog";
import { For, Show, createSignal } from "solid-js";
import type { Plan } from "../gen/maurice/v1/common_pb";
import { PlannedChange_Kind, Severity } from "../gen/maurice/v1/common_pb";
import { ApplyEvent_Status } from "../gen/maurice/v1/plans_pb";
import { plans, errorText } from "../api";

const kindLabel: Record<number, string> = {
  [PlannedChange_Kind.CREATE_USER]: "create user",
  [PlannedChange_Kind.DELETE_USER]: "delete user",
  [PlannedChange_Kind.WRITE_UNIT]: "write unit",
  [PlannedChange_Kind.REMOVE_UNIT]: "remove unit",
  [PlannedChange_Kind.START_UNIT]: "start",
  [PlannedChange_Kind.STOP_UNIT]: "stop",
  [PlannedChange_Kind.RESTART_UNIT]: "restart",
  [PlannedChange_Kind.PUBLISH_PORT]: "publish port",
  [PlannedChange_Kind.UNPUBLISH_PORT]: "unpublish port",
  [PlannedChange_Kind.SET_SECRET]: "set secret",
  [PlannedChange_Kind.BUILD_IMAGE]: "build image",
};

const sevClass: Record<number, string> = { [Severity.ERROR]: "danger", [Severity.WARNING]: "warn", [Severity.INFO]: "accent" };
const sevLabel: Record<number, string> = { [Severity.ERROR]: "error", [Severity.WARNING]: "warning", [Severity.INFO]: "info" };

type Line = { status: string; detail: string };

/**
 * Shows a plan, lets the operator apply or discard it, and streams apply events.
 * Destructive plans need an explicit confirmation checkbox (ADR-007).
 */
export function PlanDialog(props: { plan: Plan | null; onClose: () => void; onApplied?: () => void }) {
  const [confirmed, setConfirmed] = createSignal(false);
  const [events, setEvents] = createSignal<Line[]>([]);
  const [running, setRunning] = createSignal(false);
  const [done, setDone] = createSignal<"ok" | "failed" | null>(null);
  const [error, setError] = createSignal("");

  const hasErrors = () => props.plan?.findings.some((f) => f.severity === Severity.ERROR) ?? false;

  async function apply() {
    const p = props.plan;
    if (!p) return;
    setRunning(true);
    setError("");
    setEvents([]);
    try {
      let failed = false;
      for await (const ev of plans.apply({ planId: p.id, confirmed: confirmed() })) {
        const status = ev.status === ApplyEvent_Status.STARTED ? "started" : ev.status === ApplyEvent_Status.DONE ? "done" : "failed";
        if (status === "failed") failed = true;
        setEvents((e) => [...e, { status, detail: ev.detail }]);
      }
      setDone(failed ? "failed" : "ok");
      if (!failed) props.onApplied?.();
    } catch (e) {
      setError(errorText(e));
      setDone("failed");
    } finally {
      setRunning(false);
    }
  }

  async function discard() {
    const p = props.plan;
    if (!p) return;
    try {
      await plans.discard({ id: p.id });
    } catch (e) {
      setError(errorText(e));
      return;
    }
    close();
  }

  function close() {
    setConfirmed(false);
    setEvents([]);
    setDone(null);
    setError("");
    props.onClose();
  }

  return (
    <Dialog open={props.plan !== null} onOpenChange={(o) => !o && close()}>
      <Dialog.Portal>
        <Dialog.Overlay class="overlay" />
        <div class="dialog">
          <Dialog.Content class="dialog-content">
            <div class="row">
              <Dialog.Title as="h2">Plan {props.plan?.destructive ? <span class="pill danger">destructive</span> : null}</Dialog.Title>
              <span class="spacer" style={{ "margin-left": "auto" }} />
              <Dialog.CloseButton>Close</Dialog.CloseButton>
            </div>
            <Dialog.Description class="muted small mono">{props.plan?.id}</Dialog.Description>
            <Show when={props.plan}>
              {(plan) => (
                <>
                  <For each={plan().findings}>
                    {(f) => (
                      <div class="row small">
                        <span class={`pill ${sevClass[f.severity] ?? ""}`}>{sevLabel[f.severity] ?? "finding"}</span>
                        <code>{f.code}</code>
                        <span class="muted">{f.target}</span>
                        <span>{f.message}</span>
                      </div>
                    )}
                  </For>
                  <For each={plan().changes}>
                    {(c) => (
                      <div class="change">
                        <div class="row">
                          <span class={`pill ${c.destructive ? "danger" : ""}`}>{kindLabel[c.kind] ?? "change"}</span>
                          <code>{c.target}</code>
                        </div>
                        <Show when={c.diff}>
                          <pre>
                            <For each={c.diff.split("\n")}>
                              {(l) => (
                                <div class={l.startsWith("+") ? "diff-add" : l.startsWith("-") ? "diff-del" : ""}>{l}</div>
                              )}
                            </For>
                          </pre>
                        </Show>
                      </div>
                    )}
                  </For>
                  <Show when={plan().destructive && done() === null}>
                    <label style={{ "flex-direction": "row", display: "flex", gap: "8px", "align-items": "center", color: "var(--m-fg)" }}>
                      <input type="checkbox" id="confirm-destructive" checked={confirmed()} onChange={(e) => setConfirmed(e.currentTarget.checked)} />
                      I understand this removes data that cannot be restored by the panel.
                    </label>
                  </Show>
                  <Show when={events().length}>
                    <div class="events">
                      <For each={events()}>
                        {(e) => (
                          <div>
                            <span class={e.status === "failed" ? "error" : e.status === "done" ? "diff-add" : "muted"}>{e.status.padEnd(8)}</span> {e.detail}
                          </div>
                        )}
                      </For>
                    </div>
                  </Show>
                  <Show when={error()}>
                    <div class="error small">{error()}</div>
                  </Show>
                  <div class="row">
                    <Show
                      when={done() === null}
                      fallback={
                        <>
                          <span class={done() === "ok" ? "diff-add" : "error"}>{done() === "ok" ? "Applied" : "Apply failed"}</span>
                          <button onClick={close}>Close</button>
                        </>
                      }
                    >
                      <button class="primary" disabled={running() || hasErrors() || (plan().destructive && !confirmed())} onClick={apply}>
                        {running() ? "Applying…" : "Apply"}
                      </button>
                      <button disabled={running()} onClick={discard}>Discard</button>
                      <Show when={hasErrors()}>
                        <span class="error small">Fix the errors above, then plan again.</span>
                      </Show>
                    </Show>
                  </div>
                </>
              )}
            </Show>
          </Dialog.Content>
        </div>
      </Dialog.Portal>
    </Dialog>
  );
}
