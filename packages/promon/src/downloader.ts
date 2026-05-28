export async function downloadBinary(): Promise<never> {
  throw new Error("GitHub Release binary downloads are implemented in the release phase.");
}

