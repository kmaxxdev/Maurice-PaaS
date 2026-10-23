import { createClient, ConnectError } from "@connectrpc/connect";
import { createConnectTransport } from "@connectrpc/connect-web";
import { Workspaces } from "./gen/maurice/v1/workspaces_pb";
import { Plans } from "./gen/maurice/v1/plans_pb";
import { Services } from "./gen/maurice/v1/services_pb";
import { Secrets } from "./gen/maurice/v1/secrets_pb";
import { Projects } from "./gen/maurice/v1/projects_pb";

const transport = createConnectTransport({ baseUrl: "/" });

export const workspaces = createClient(Workspaces, transport);
export const plans = createClient(Plans, transport);
export const services = createClient(Services, transport);
export const secrets = createClient(Secrets, transport);
export const projects = createClient(Projects, transport);

/** Human-readable error text from a Connect failure. */
export function errorText(e: unknown): string {
  if (e instanceof ConnectError) return e.rawMessage || e.message;
  if (e instanceof Error) return e.message;
  return String(e);
}
