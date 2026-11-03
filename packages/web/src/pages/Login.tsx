import { useNavigate, useSearchParams } from "@solidjs/router";
import { Show, createSignal } from "solid-js";
import { auth, errorText } from "../api";

export function Login() {
  const navigate = useNavigate();
  const [params] = useSearchParams();
  const [login, setLogin] = createSignal("");
  const [password, setPassword] = createSignal("");
  const [error, setError] = createSignal("");
  const [busy, setBusy] = createSignal(false);

  async function submit(e: Event) {
    e.preventDefault();
    setError("");
    setBusy(true);
    try {
      await auth.login({ login: login(), password: password() });
      const next = typeof params.next === "string" && params.next.startsWith("/") ? params.next : "/";
      navigate(next, { replace: true });
    } catch (err) {
      setError(errorText(err));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div style={{ display: "grid", "place-items": "center", "min-height": "60vh" }}>
      <form class="card" onSubmit={submit} style={{ display: "grid", gap: "12px", width: "min(360px, 100%)" }}>
        <h2>Sign in</h2>
        <label>
          Login
          <input id="login" value={login()} onInput={(e) => setLogin(e.currentTarget.value)} autocomplete="username" required autofocus />
        </label>
        <label>
          Password
          <input id="password" type="password" value={password()} onInput={(e) => setPassword(e.currentTarget.value)} autocomplete="current-password" required />
        </label>
        <Show when={error()}>
          <div class="error small">{error()}</div>
        </Show>
        <button class="primary" type="submit" disabled={busy()}>{busy() ? "Signing in…" : "Sign in"}</button>
        <p class="muted small" style={{ margin: 0 }}>
          The first administrator's password is printed by <code>maurice host prepare</code>; <code>maurice host password-reset admin</code> prints a new one.
        </p>
      </form>
    </div>
  );
}
