import { createReadStream, stat } from "node:fs";
import { createServer } from "node:http";
import path from "node:path";
import { fileURLToPath } from "node:url";

const port = Number(process.argv[2] || 8090);
const host = process.argv[3] || "0.0.0.0";
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");

const mime = new Map([
  [".html", "text/html; charset=utf-8"],
  [".css", "text/css; charset=utf-8"],
  [".js", "application/javascript; charset=utf-8"],
  [".png", "image/png"],
  [".jpg", "image/jpeg"],
  [".jpeg", "image/jpeg"],
  [".svg", "image/svg+xml"],
  [".zip", "application/zip"],
  [".md", "text/markdown; charset=utf-8"],
]);

const server = createServer((request, response) => {
  let pathname = decodeURIComponent(new URL(request.url || "/", `http://${host}`).pathname);
  if (pathname === "/") {
    pathname = "/promo-site/index.html";
  }

  const filePath = path.normalize(path.join(root, pathname));
  if (!filePath.startsWith(root)) {
    response.writeHead(403);
    response.end("Forbidden");
    return;
  }

  stat(filePath, (error, info) => {
    if (error || !info.isFile()) {
      response.writeHead(404);
      response.end("Not found");
      return;
    }

    response.writeHead(200, {
      "Content-Type": mime.get(path.extname(filePath).toLowerCase()) || "application/octet-stream",
    });
    createReadStream(filePath).pipe(response);
  });
});

server.listen(port, host, () => {
  console.log(`ArtForgeStudio promo site: http://${host}:${port}/promo-site/index.html`);
});
