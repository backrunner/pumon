import http from "node:http";

const server = http.createServer((_, response) => {
  response.end(process.env.PROCWATCH_WORKER_ID || "0");
});

server.listen(0, "127.0.0.1");

function shutdown() {
  server.close(() => process.exit(0));
}

process.on("disconnect", shutdown);
process.on("SIGTERM", shutdown);

setInterval(() => {}, 1000);
