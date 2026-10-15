const PERSISTED = new Set(["sessions", "conversation", "labels"]);

/** Which successful queries are written to the IndexedDB cache. Search and
 *  archive pages are one-off views, not worth restoring on reload. */
export function shouldPersistQuery(q: {
  queryKey: readonly unknown[];
  state: { status: string };
}): boolean {
  const [root, sub] = q.queryKey;
  return (
    q.state.status === "success" &&
    PERSISTED.has(root as string) &&
    !(root === "sessions" && sub === "search")
  );
}
