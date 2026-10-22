import { createClient } from "@connectrpc/connect";
import { createConnectTransport } from "@connectrpc/connect-web";
import { Workspaces } from "./gen/maurice/v1/workspaces_pb";
import { Plans } from "./gen/maurice/v1/plans_pb";

const transport = createConnectTransport({ baseUrl: "/" });

export const workspaces = createClient(Workspaces, transport);
export const plans = createClient(Plans, transport);
