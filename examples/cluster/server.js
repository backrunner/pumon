const http = require("node:http");

http.createServer((_, res) => {
  res.end("cluster-placeholder");
}).listen(Number(process.env.PORT || 3104));

