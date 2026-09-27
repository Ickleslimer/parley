import { createServer } from "vite";
import { fileURLToPath } from "node:url";

const server = await createServer({
  configFile: fileURLToPath(new URL("../vite.config.ts", import.meta.url)),
  root: fileURLToPath(new URL("..", import.meta.url)),
  plugins: [
    {
      name: "fixture-shutdown",
      configureServer(viteServer) {
        viteServer.middlewares.use("/__fixture_shutdown", (_request, response) => {
          response.statusCode = 204;
          response.end();
          setImmediate(async () => {
            await viteServer.close();
            process.exit(0);
          });
        });
      },
    },
  ],
  server: {
    host: "127.0.0.1",
    port: 1420,
    strictPort: true,
  },
});

await server.listen();
process.stdout.write("fixture-server-ready\n");
