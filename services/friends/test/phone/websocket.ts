// A tiny server-side WebSocket for the phone tests, so the mock can stand in for
// the edge's `/v1/live` room without another dependency. It speaks just enough
// of RFC 6455 for the test client: a handshake, text frames both ways, ping and
// close. No binary frames, no extensions.
import { createHash } from 'node:crypto';
import type { IncomingMessage } from 'node:http';
import type { Socket } from 'node:net';

const GUID = '258EAFA5-E914-47DA-95CA-C5AB0DC85B11';

export class TestSocket {
  private buffer = Buffer.alloc(0);
  private fragments: Buffer[] = [];
  closed = false;

  private constructor(private readonly socket: Socket, private readonly onText: (text: string) => void) {
    socket.on('data', chunk => {
      this.buffer = Buffer.concat([this.buffer, Buffer.from(chunk)]);
      this.read();
    });
    socket.on('close', () => { this.closed = true; });
    socket.on('error', () => { this.closed = true; });
  }

  /** Completes the handshake and starts listening. Null when it is not a WebSocket. */
  static accept(request: IncomingMessage, socket: Socket, onText: (text: string) => void): TestSocket | null {
    const key = request.headers['sec-websocket-key'];
    if (typeof key !== 'string') {
      socket.destroy();
      return null;
    }
    const accept = createHash('sha1').update(key + GUID).digest('base64');
    socket.write([
      'HTTP/1.1 101 Switching Protocols',
      'Upgrade: websocket',
      'Connection: Upgrade',
      `Sec-WebSocket-Accept: ${accept}`,
      '', '',
    ].join('\r\n'));
    return new TestSocket(socket, onText);
  }

  send(value: unknown): void {
    this.frame(0x1, Buffer.from(JSON.stringify(value), 'utf8'));
  }

  close(): void {
    if (this.closed) return;
    this.frame(0x8, Buffer.alloc(0));
    this.closed = true;
    try { this.socket.end(); } catch { /* gone already */ }
  }

  private frame(opcode: number, payload: Buffer): void {
    if (this.closed) return;
    const length = payload.length;
    let header: Buffer;
    if (length < 126) {
      header = Buffer.from([0x80 | opcode, length]);
    } else if (length < 65536) {
      header = Buffer.alloc(4);
      header[0] = 0x80 | opcode;
      header[1] = 126;
      header.writeUInt16BE(length, 2);
    } else {
      header = Buffer.alloc(10);
      header[0] = 0x80 | opcode;
      header[1] = 127;
      header.writeBigUInt64BE(BigInt(length), 2);
    }
    try { this.socket.write(Buffer.concat([header, payload])); } catch { this.closed = true; }
  }

  private read(): void {
    while (this.buffer.length >= 2) {
      const first = this.buffer[0]!;
      const second = this.buffer[1]!;
      const fin = (first & 0x80) !== 0;
      const opcode = first & 0x0f;
      const masked = (second & 0x80) !== 0;
      let length = second & 0x7f;
      let offset = 2;
      if (length === 126) {
        if (this.buffer.length < 4) return;
        length = this.buffer.readUInt16BE(2);
        offset = 4;
      } else if (length === 127) {
        if (this.buffer.length < 10) return;
        length = Number(this.buffer.readBigUInt64BE(2));
        offset = 10;
      }
      const maskStart = offset;
      if (masked) offset += 4;
      if (this.buffer.length < offset + length) return;
      const payload = Buffer.from(this.buffer.subarray(offset, offset + length));
      if (masked) {
        const mask = this.buffer.subarray(maskStart, maskStart + 4);
        for (let i = 0; i < payload.length; i++) payload[i] = payload[i]! ^ mask[i % 4]!;
      }
      this.buffer = this.buffer.subarray(offset + length);
      if (opcode === 0x8) { this.close(); return; }
      if (opcode === 0x9) { this.frame(0xa, payload); continue; }
      if (opcode === 0xa) continue;
      if (opcode === 0x1 || opcode === 0x0) {
        this.fragments.push(payload);
        if (fin) {
          const text = Buffer.concat(this.fragments).toString('utf8');
          this.fragments = [];
          if (text) this.onText(text);
        }
        continue;
      }
      // Binary frames and anything else are not part of this protocol.
    }
  }
}
