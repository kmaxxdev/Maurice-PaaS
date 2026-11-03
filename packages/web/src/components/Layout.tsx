import { A, useLocation, useNavigate } from "@solidjs/router";
import { Show, createResource, type ParentProps } from "solid-js";
import { auth } from "../api";
import { Role } from "../gen/maurice/v1/auth_pb";

export function roleLabel(r: Role): string {
  switch (r) {
    case Role.ADMIN:
      return "admin";
    case Role.OPERATOR:
      return "operator";
    case Role.VIEWER:
      return "viewer";
    default:
      return "";
  }
}

export function Layout(props: ParentProps) {
  const loc = useLocation();
  const navigate = useNavigate();
  const onLogin = () => loc.pathname.startsWith("/login");
  const [me, { refetch }] = createResource(
    () => loc.pathname,
    async (path) => (path.startsWith("/login") ? undefined : auth.me({})),
  );

  async function logout() {
    try {
      await auth.logout({});
    } finally {
      refetch();
      navigate("/login", { replace: true });
    }
  }

  return (
    <>
      <header class="top">
        <A href="/" class="brand">Maurice</A>
        <nav class="crumbs" aria-label="Breadcrumb">
          <span>{loc.pathname === "/" ? "workspaces" : ""}</span>
        </nav>
        <span class="spacer" />
        <Show when={!onLogin() && me()}>
          {(u) => (
            <>
              <A href="/settings" class="muted small">{u().login} · {roleLabel(u().role)}</A>
              <button onClick={logout}>Sign out</button>
            </>
          )}
        </Show>
        <span class="muted small">alpha</span>
      </header>
      <main class="page">{props.children}</main>
    </>
  );
}
