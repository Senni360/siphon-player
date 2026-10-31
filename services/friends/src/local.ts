import { createServer } from 'node:http';
import handler from './handler.js';

const port = Number(process.env.PORT ?? 8787);
createServer(handler).listen(port, '127.0.0.1', () => {
  console.log(`Friends service listening on http://127.0.0.1:${port}/api`);
});
