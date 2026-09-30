export interface Principal {
  userId: string;
}

export interface AuthResolver {
  resolve: (token: string) => Promise<Principal | null>;
}

export function createStaticResolver(tokens: Map<string, string>): AuthResolver {
  return {
    async resolve(token: string): Promise<Principal | null> {
      const userId = tokens.get(token);
      return userId ? { userId } : null;
    },
  };
}

export function parseStaticTokens(raw: string | undefined): Map<string, string> {
  const map = new Map<string, string>();
  if (!raw) return map;
  for (const pair of raw.split(",")) {
    const idx = pair.indexOf(":");
    if (idx === -1) continue;
    const token = pair.slice(0, idx).trim();
    const userId = pair.slice(idx + 1).trim();
    if (token && userId) map.set(token, userId);
  }
  return map;
}
