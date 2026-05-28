import http from "node:http";

const port = Number(process.env.PORT || 3103);
http.createServer((_, res) => {
  res.end("package-script");
}).listen(port, () => {
  console.log(`package-script listening on ${port}`);
});

