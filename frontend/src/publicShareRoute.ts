export type PublicShareRoute = {
  clientKey: string | null;
  secret: string;
  shareId: string;
};

export function parsePublicShareRouteHash(
  hash: string,
): PublicShareRoute | null {
  const match = hash.match(
    /^#\/share\/([^/]+)\/([^/]+)(?:\/vps\/([^/]+))?$/,
  );
  if (!match) return null;
  try {
    return {
      clientKey: match[3] ? decodeURIComponent(match[3]) : null,
      secret: decodeURIComponent(match[2]),
      shareId: decodeURIComponent(match[1]),
    };
  } catch {
    return null;
  }
}
