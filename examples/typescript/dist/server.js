import http from "node:http";
const port = Number(process.env.PORT || 3102);
http.createServer((_, res) => {
    res.end("typescript-prebuilt");
}).listen(port, () => {
    console.log(`typescript-prebuilt listening on ${port}`);
});

