export function releaseChannel(version: string): "stable" | "beta" | "alpha" {
  if (version.includes("-alpha.")) return "alpha";
  if (version.includes("-beta.")) return "beta";
  return "stable";
}

