// A round-robin load balancer with no affinity: every request goes to the next instance, so a
// session or a stream that needs "its" instance shows up as a failure. Serves its hit counts at
// /__lb_hits.
//   node lb.mjs <listen port> <instance port>...
import http from "node:http";
const ports = process.argv.slice(3).map(Number);
const listen = Number(process.argv[2]);
let i = 0;
const hits = ports.map(() => 0);
http.createServer((req, res) => {
  if (req.url === "/__lb_hits") { res.end(JSON.stringify(hits)); return; }
  const n = i++ % ports.length; hits[n]++;
  const p = http.request({ host: "127.0.0.1", port: ports[n], path: req.url, method: req.method, headers: { ...req.headers, "x-lb-instance": String(n) }, agent: false }, (r) => {
    res.writeHead(r.statusCode, { ...r.headers, "x-served-by": String(ports[n]) });
    r.pipe(res);
  });
  p.on("error", (e) => { res.writeHead(502); res.end("bad gateway " + e.message); });
  req.pipe(p);
}).listen(listen, "127.0.0.1");
