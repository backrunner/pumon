const http = require("node:http");

const port = Number(process.env.PORT || 3101);
http.createServer((_, res) => {
  res.end("basic-js");
}).listen(port, () => {
  console.log(`basic-js listening on ${port}`);
});

