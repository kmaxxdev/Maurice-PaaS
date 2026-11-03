import { createClient, ConnectError, Code, type Interceptor } from "@connectrpc/connect";
import { createConnectTransport } from "@connectrpc/connect-web";
import { Workspaces } from "./gen/maurice/v1/workspaces_pb";
import { Plans } from "./gen/maurice/v1/plans_pb";
import { Services } from "./gen/maurice/v1/services_pb";
import { Secrets } from "./gen/maurice/v1/secrets_pb";
import { Projects } from "./gen/maurice/v1/projects_pb";
import { Auth } from "./gen/maurice/v1/auth_pb";
import { Edge } from "./gen/maurice/v1/edge_pb";

/** An expired or missing session sends the browser to the login page, remembering where it was. */
const requireLogin: Interceptor = (next) => async (req) => {
  try {
    return await next(req);
  } catch (e) {
    if (e instanceof ConnectError && e.code === Code.Unauthenticated && !location.pathname.startsWith("/login")) {
      location.assign(`/login?next=${encodeURIComponent(location.pathname)}`);
    }
    throw e;
  }
};

const transport = createConnectTransport({ baseUrl: "/", interceptors: [requireLogin] });

export const workspaces = createClient(Workspaces, transport);
export const plans = createClient(Plans, transport);
export const services = createClient(Services, transport);
export const secrets = createClient(Secrets, transport);
export const projects = createClient(Projects, transport);
export const auth = createClient(Auth, transport);
export const edge = createClient(Edge, transport);

/** Human-readable error text from a Connect failure. */
export function errorText(e: unknown): string {
  if (e instanceof ConnectError) return e.rawMessage || e.message;
  if (e instanceof Error) return e.message;
  return String(e);
}
