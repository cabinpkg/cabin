const http = require("http");
const fs = require("fs");
const [port, dumpPath] = process.argv.slice(2);
const exportPath = /^\/accounts\/[0-9a-f]{32}\/d1\/database\/[0-9a-f-]{36}\/export$/;
http.createServer((req, res) => {
  if (req.method === "POST" && exportPath.test(req.url)) {
    res.setHeader("content-type", "application/json");
    res.end(JSON.stringify({ success: true, result: { status: "complete",
      at_bookmark: "smoke",
      result: { signed_url: `http://127.0.0.1:${port}/dump.sql`, filename: "dump.sql" } } }));
  } else if (req.method === "GET" && req.url === "/dump.sql") {
    const dump = fs.readFileSync(dumpPath);
    res.setHeader("content-length", dump.length);
    res.end(dump);
  } else {
    res.statusCode = 404;
    // text/plain: the echoed URL is an operator diagnostic, never a
    // page (CodeQL js/reflected-xss); no smoke leg reads this body.
    res.setHeader("content-type", "text/plain");
    res.end(`unexpected request: ${req.method} ${req.url}`);
  }
}).listen(port, "127.0.0.1");
