import { For, Show, createResource, createSignal } from "solid-js";
import { Role } from "../gen/maurice/v1/auth_pb";
import { Scope } from "../gen/maurice/v1/common_pb";
import { auth, workspaces, errorText } from "../api";
import { roleLabel } from "../components/Layout";

const scopeLabel: Record<number, string> = { [Scope.READ]: "read", [Scope.PLAN]: "plan", [Scope.APPLY]: "apply" };
const fmt = (t?: { seconds: bigint }) => (t && t.seconds ? new Date(Number(t.seconds) * 1000).toLocaleString() : "—");

/** Tokens for the CLI and bots, users (admins only) and the audit log. */
export function Settings() {
  const [me] = createResource(() => auth.me({}));
  const isAdmin = () => me()?.role === Role.ADMIN;
  const [tokens, { refetch: refetchTokens }] = createResource(async () => (await auth.listTokens({})).tokens);
  const [users, { refetch: refetchUsers }] = createResource(me, async (u) => (u.role === Role.ADMIN ? (await auth.listUsers({})).users : []));
  const [audit, { refetch: refetchAudit }] = createResource(async () => (await auth.listAudit({ limit: 100 })).entries);
  const [wsList] = createResource(async () => (await workspaces.list({})).workspaces);

  // --- tokens ---
  const [tName, setTName] = createSignal("");
  const [tScope, setTScope] = createSignal<Scope>(Scope.APPLY);
  const [tHuman, setTHuman] = createSignal(true);
  const [tDays, setTDays] = createSignal("0");
  const [tWs, setTWs] = createSignal<string[]>([]);
  const [secret, setSecret] = createSignal("");
  const [tMsg, setTMsg] = createSignal("");

  async function createToken(e: Event) {
    e.preventDefault();
    setTMsg("");
    setSecret("");
    try {
      const scopes = tScope() === Scope.APPLY ? [Scope.READ, Scope.PLAN, Scope.APPLY] : tScope() === Scope.PLAN ? [Scope.READ, Scope.PLAN] : [Scope.READ];
      const r = await auth.createToken({ name: tName(), scopes, workspaceIds: tWs(), human: tHuman(), expiresDays: Number(tDays()) || 0 });
      setSecret(r.secret);
      setTName("");
      refetchTokens();
      refetchAudit();
    } catch (err) {
      setTMsg(errorText(err));
    }
  }

  async function revoke(id: string) {
    setTMsg("");
    try {
      await auth.revokeToken({ id });
      refetchTokens();
      refetchAudit();
    } catch (err) {
      setTMsg(errorText(err));
    }
  }

  // --- users ---
  const [uLogin, setULogin] = createSignal("");
  const [uPassword, setUPassword] = createSignal("");
  const [uRole, setURole] = createSignal<Role>(Role.OPERATOR);
  const [uMsg, setUMsg] = createSignal("");

  async function createUser(e: Event) {
    e.preventDefault();
    setUMsg("");
    try {
      await auth.createUser({ login: uLogin(), password: uPassword(), role: uRole() });
      setULogin("");
      setUPassword("");
      refetchUsers();
      refetchAudit();
    } catch (err) {
      setUMsg(errorText(err));
    }
  }

  async function updateUser(id: string, role: Role, disabled: boolean) {
    setUMsg("");
    try {
      await auth.updateUser({ id, role, disabled });
      refetchUsers();
      refetchAudit();
    } catch (err) {
      setUMsg(errorText(err));
    }
  }

  return (
    <>
      <div class="row">
        <h2>Settings</h2>
        <Show when={me()}>{(u) => <span class="muted small">{u().login} · {roleLabel(u().role)}</span>}</Show>
      </div>

      <div class="section card">
        <h2 style={{ "margin-bottom": "6px" }}>API tokens</h2>
        <p class="muted small" style={{ margin: "0 0 10px" }}>
          For <code>marik</code>, MCP and bots. The secret is shown once. A token can never do more than its owner's role; a non-human token never applies in prod or staging.
        </p>
        <Show when={tokens()?.length} fallback={<p class="muted small">No tokens.</p>}>
          <table>
            <thead><tr><th>Name</th><th>Owner</th><th>Scopes</th><th>Workspaces</th><th>Human</th><th>Expires</th><th>Last used</th><th /></tr></thead>
            <tbody>
              <For each={tokens()}>
                {(t) => (
                  <tr>
                    <td><code>{t.name}</code></td>
                    <td>{t.owner}</td>
                    <td>{t.scopes.map((s) => scopeLabel[s]).join(", ")}</td>
                    <td class="small">{t.workspaceIds.length ? t.workspaceIds.map((id) => wsList()?.find((w) => w.id === id)?.name ?? id.slice(0, 8)).join(", ") : "all"}</td>
                    <td>{t.human ? "yes" : "no"}</td>
                    <td class="small">{fmt(t.expiresAt)}</td>
                    <td class="small">{fmt(t.lastUsedAt)}</td>
                    <td><button class="danger" onClick={() => revoke(t.id)}>Revoke</button></td>
                  </tr>
                )}
              </For>
            </tbody>
          </table>
        </Show>
        <form class="inline" onSubmit={createToken} style={{ "margin-top": "10px" }}>
          <label>
            Name
            <input id="tok-name" value={tName()} onInput={(e) => setTName(e.currentTarget.value)} placeholder="ci-deploy" required />
          </label>
          <label>
            Scope
            <select id="tok-scope" value={String(tScope())} onChange={(e) => setTScope(Number(e.currentTarget.value) as Scope)}>
              <option value={String(Scope.READ)}>read</option>
              <option value={String(Scope.PLAN)}>read + plan</option>
              <option value={String(Scope.APPLY)}>read + plan + apply</option>
            </select>
          </label>
          <label>
            Workspaces (empty = all)
            <select id="tok-ws" multiple size="3" onChange={(e) => setTWs(Array.from(e.currentTarget.selectedOptions).map((o) => o.value))}>
              <For each={wsList()}>{(w) => <option value={w.id}>{w.name}</option>}</For>
            </select>
          </label>
          <label>
            Expires in days (0 = never)
            <input id="tok-days" value={tDays()} onInput={(e) => setTDays(e.currentTarget.value)} inputmode="numeric" style={{ width: "90px" }} />
          </label>
          <label style={{ "flex-direction": "row", display: "flex", gap: "6px", "align-items": "center" }}>
            <input type="checkbox" id="tok-human" checked={tHuman()} onChange={(e) => setTHuman(e.currentTarget.checked)} /> human behind it
          </label>
          <button class="primary" type="submit">Create</button>
          <Show when={tMsg()}><span class="error small">{tMsg()}</span></Show>
        </form>
        <Show when={secret()}>
          <div class="card" style={{ "margin-top": "10px" }}>
            <div class="small">Copy it now; it is not stored:</div>
            <pre id="tok-secret">{secret()}</pre>
            <div class="muted small" style={{ "margin-top": "6px" }}>Use it as <code>MAURICE_TOKEN</code> or put it in <code>~/.config/maurice/token</code>.</div>
          </div>
        </Show>
      </div>

      <Show when={isAdmin()}>
        <div class="section card">
          <h2 style={{ "margin-bottom": "6px" }}>Users</h2>
          <p class="muted small" style={{ margin: "0 0 10px" }}>viewer reads · operator plans everywhere and applies in dev · admin does everything, including users and environments.</p>
          <table>
            <thead><tr><th>Login</th><th>Role</th><th>State</th><th>Since</th><th /></tr></thead>
            <tbody>
              <For each={users()}>
                {(u) => (
                  <tr>
                    <td><code>{u.login}</code></td>
                    <td>
                      <select value={String(u.role)} onChange={(e) => updateUser(u.id, Number(e.currentTarget.value) as Role, u.disabled)} disabled={u.id === me()?.id}>
                        <option value={String(Role.ADMIN)}>admin</option>
                        <option value={String(Role.OPERATOR)}>operator</option>
                        <option value={String(Role.VIEWER)}>viewer</option>
                      </select>
                    </td>
                    <td>{u.disabled ? <span class="pill danger">disabled</span> : <span class="pill ok">active</span>}</td>
                    <td class="small">{fmt(u.createdAt)}</td>
                    <td>
                      <Show when={u.id !== me()?.id}>
                        <button onClick={() => updateUser(u.id, u.role, !u.disabled)}>{u.disabled ? "Enable" : "Disable"}</button>
                      </Show>
                    </td>
                  </tr>
                )}
              </For>
            </tbody>
          </table>
          <form class="inline" onSubmit={createUser} style={{ "margin-top": "10px" }}>
            <label>
              Login
              <input id="user-login" value={uLogin()} onInput={(e) => setULogin(e.currentTarget.value)} required pattern="[a-z0-9][a-z0-9._-]*" />
            </label>
            <label>
              Password (12+ characters)
              <input id="user-password" type="password" value={uPassword()} onInput={(e) => setUPassword(e.currentTarget.value)} required minlength="12" autocomplete="new-password" />
            </label>
            <label>
              Role
              <select id="user-role" value={String(uRole())} onChange={(e) => setURole(Number(e.currentTarget.value) as Role)}>
                <option value={String(Role.ADMIN)}>admin</option>
                <option value={String(Role.OPERATOR)}>operator</option>
                <option value={String(Role.VIEWER)}>viewer</option>
              </select>
            </label>
            <button class="primary" type="submit">Create user</button>
            <Show when={uMsg()}><span class="error small">{uMsg()}</span></Show>
          </form>
        </div>
      </Show>

      <div class="section card">
        <div class="row" style={{ "margin-bottom": "10px" }}>
          <h2>Audit log</h2>
          <span class="muted small">last 100 state-changing calls</span>
          <span style={{ "margin-left": "auto" }} />
          <button onClick={() => refetchAudit()}>Refresh</button>
        </div>
        <Show when={audit()?.length} fallback={<p class="muted small">Nothing yet.</p>}>
          <table>
            <thead><tr><th>When</th><th>Who</th><th>Action</th><th>Target</th><th>Tier</th><th>Outcome</th><th>Detail</th></tr></thead>
            <tbody>
              <For each={audit()}>
                {(a) => (
                  <tr>
                    <td class="small mono">{fmt(a.at)}</td>
                    <td class="small">{a.principal}</td>
                    <td class="mono small">{a.action}</td>
                    <td class="mono small">{a.target}</td>
                    <td class="small">{a.tier}</td>
                    <td><span class={`pill ${a.outcome === "ok" ? "ok" : "danger"}`}>{a.outcome}</span></td>
                    <td class="small muted">{a.detail}</td>
                  </tr>
                )}
              </For>
            </tbody>
          </table>
        </Show>
      </div>
    </>
  );
}
