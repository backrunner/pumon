#!/usr/bin/env node
// Install-time downloads are intentionally best-effort. The bin wrapper also
// downloads on first run so npm --ignore-scripts remains supported.
console.log("Procwatch will resolve its native binary on first run if no local cargo build exists.");
