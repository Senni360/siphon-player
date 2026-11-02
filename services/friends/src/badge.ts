import { readFileSync } from 'node:fs';

export type Rgb = readonly [number, number, number];

// Darker than this is the badge's plate and rim, which stay as they are.
const PLATE = 0.27;
const RING = 0.38;
// The brightest grey in the artwork, where a lit ring reaches the full colour.
const LIT = 0.97;

const ramp = (from: number, to: number, value: number) => {
  const t = Math.min(1, Math.max(0, (value - from) / (to - from)));
  return t * t * (3 - 2 * t);
};

function tintEntry(table: Uint8Array, at: number, colour: Rgb) {
  const level = Math.max(table[at], table[at + 1], table[at + 2]) / 255;
  const amount = ramp(PLATE, RING, level);
  const brightness = Math.min(1, level / LIT);
  for (let channel = 0; channel < 3; channel++) {
    const tinted = colour[channel] * brightness;
    table[at + channel] = Math.round(table[at + channel] * (1 - amount) + tinted * amount);
  }
}

// The artwork is greys, so its rings take a colour by changing the colour tables
// alone: every frame, its timing and its transparency stay byte for byte.
export function tintGif(source: Uint8Array, colour: Rgb): Uint8Array {
  const gif = Uint8Array.from(source);
  const tint = (start: number, size: number) => {
    for (let entry = 0; entry < size; entry++) tintEntry(gif, start + entry * 3, colour);
    return start + size * 3;
  };
  const tableSize = (flags: number) => 2 << (flags & 7);
  const skipBlocks = (start: number) => {
    let at = start;
    while (gif[at] !== 0) at += gif[at] + 1;
    return at + 1;
  };
  let at = 13;
  if (gif[10] & 0x80) at = tint(at, tableSize(gif[10]));
  while (at < gif.length && gif[at] !== 0x3b) {
    if (gif[at] === 0x21) at = skipBlocks(at + 2);
    else if (gif[at] === 0x2c) {
      const flags = gif[at + 9];
      at += 10;
      if (flags & 0x80) at = tint(at, tableSize(flags));
      at = skipBlocks(at + 1);
    } else throw new Error('Not a GIF block');
  }
  return gif;
}

let artwork: Uint8Array | undefined;

// A colour from three hex digits, so there are only ever 4096 of these images.
export function badge(hex: string): Response {
  artwork ??= readFileSync(new URL('../assets/encore-badge.gif', import.meta.url));
  const colour = [...hex].map(digit => parseInt(digit, 16) * 17) as unknown as Rgb;
  return new Response(Buffer.from(tintGif(artwork, colour)), {
    headers: {
      'Content-Type': 'image/gif',
      'Cache-Control': 'public, max-age=31536000, immutable',
      'X-Content-Type-Options': 'nosniff',
    },
  });
}
