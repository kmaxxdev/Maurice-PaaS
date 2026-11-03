import { Env } from "./gen/maurice/v1/common_pb";

/** Environments in the order the UI shows them. */
export const ENV_ORDER: Env[] = [Env.DEV, Env.STAGING, Env.PROD];

export function envLabel(e: Env): string {
  switch (e) {
    case Env.DEV:
      return "dev";
    case Env.STAGING:
      return "staging";
    case Env.PROD:
      return "prod";
    default:
      return "shared";
  }
}

/** Pill tone: prod stands out, staging is marked, dev is plain. */
export function envTone(e: Env): string {
  switch (e) {
    case Env.PROD:
      return "warn";
    case Env.STAGING:
      return "accent";
    default:
      return "";
  }
}

export function parseEnv(s: string): Env {
  switch (s) {
    case "dev":
      return Env.DEV;
    case "staging":
      return Env.STAGING;
    case "prod":
      return Env.PROD;
    default:
      return Env.UNSPECIFIED;
  }
}
