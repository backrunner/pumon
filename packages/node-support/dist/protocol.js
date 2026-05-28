export function writeJson(value) {
    process.stdout.write(`${JSON.stringify(value)}\n`);
}
export function fail(message) {
    process.stderr.write(`${JSON.stringify({ error: message })}\n`);
    process.exit(1);
}

