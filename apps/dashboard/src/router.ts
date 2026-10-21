import { useSyncExternalStore } from "react";
export type RouteName =
  | "brief"
  | "detail"
  | "health"
  | "overview"
  | "operations"
  | "workplans"
  | "coordination"
  | "commands"
  | "intake"
  | "incidents"
  | "models"
  | "git"
  | "atlas";
const names = new Set<RouteName>([
  "brief",
  "detail",
  "health",
  "overview",
  "operations",
  "workplans",
  "coordination",
  "commands",
  "intake",
  "incidents",
  "models",
  "git",
  "atlas",
]);
export function parseRoute(hash: string): {
  name: RouteName;
  params: URLSearchParams;
} {
  const [path, query = ""] = hash.replace(/^#\/?/, "").split("?");
  const alias =
    path === "fleet-map" ? "atlas" : path === "fleet" ? "detail" : path;
  return {
    name: names.has(alias as RouteName) ? (alias as RouteName) : "detail",
    params: new URLSearchParams(query),
  };
}
export function routeHref(
  name: RouteName,
  values: Record<string, string | null | undefined> = {},
): string {
  const query = new URLSearchParams(
    Object.entries(values).filter(
      (entry): entry is [string, string] =>
        typeof entry[1] === "string" && !!entry[1],
    ),
  );
  return `#/${name}${query.size ? `?${query}` : ""}`;
}
export function navigate(
  name: RouteName,
  values: Record<string, string | null | undefined> = {},
) {
  window.location.hash = routeHref(name, values);
}
const subscribe = (notify: () => void) => {
  window.addEventListener("hashchange", notify);
  return () => window.removeEventListener("hashchange", notify);
};
export function useRoute() {
  return parseRoute(
    useSyncExternalStore(
      subscribe,
      () => window.location.hash,
      () => "#/detail",
    ),
  );
}
