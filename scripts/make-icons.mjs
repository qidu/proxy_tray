// Regenerates src-tauri/icons/*.png.
//
// The tray icon is the app's only status surface when the window is hidden, so
// the three states must be distinguishable at icon size: they are one hollow
// circle each, differing only in colour (design doc §5).
//
// Run: node scripts/make-icons.mjs

import { deflateSync } from 'node:zlib';
import { writeFileSync, mkdirSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const ICON_DIR = join(dirname(fileURLToPath(import.meta.url)), '..', 'src-tauri', 'icons');

const CRC_TABLE = (() => {
  const table = new Uint32Array(256);
  for (let n = 0; n < 256; n++) {
    let c = n;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    table[n] = c >>> 0;
  }
  return table;
})();

function crc32(buf) {
  let c = 0xffffffff;
  for (const byte of buf) c = CRC_TABLE[(c ^ byte) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
}

function chunk(type, data) {
  const head = Buffer.alloc(4);
  head.writeUInt32BE(data.length, 0);
  const body = Buffer.concat([Buffer.from(type, 'ascii'), data]);
  const crc = Buffer.alloc(4);
  crc.writeUInt32BE(crc32(body), 0);
  return Buffer.concat([head, body, crc]);
}

/** Encode an RGBA byte buffer as a PNG. */
function encodePng(rgba, size) {
  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(size, 0);
  ihdr.writeUInt32BE(size, 4);
  ihdr[8] = 8; // bit depth
  ihdr[9] = 6; // colour type: RGBA
  ihdr[10] = 0; // deflate
  ihdr[11] = 0; // adaptive filtering
  ihdr[12] = 0; // no interlace

  // Each scanline is prefixed with its filter byte (0 = none).
  const raw = Buffer.alloc(size * (size * 4 + 1));
  for (let y = 0; y < size; y++) {
    const src = y * size * 4;
    const dst = y * (size * 4 + 1) + 1;
    rgba.copy(raw, dst, src, src + size * 4);
  }

  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk('IHDR', ihdr),
    chunk('IDAT', deflateSync(raw, { level: 9 })),
    chunk('IEND', Buffer.alloc(0)),
  ]);
}

/**
 * A hollow circle (ring), anti-aliased by treating the distance from the centre
 * as pixel coverage. The stroke is a fixed fraction of the size so the ring
 * keeps the same weight at every resolution. One pixel of feathering is enough
 * at 32px.
 */
function ring(size, [r, g, b]) {
  const rgba = Buffer.alloc(size * size * 4);
  const c = (size - 1) / 2;
  const outer = size / 2 - 1;
  const stroke = Math.max(2, Math.round(size * 0.12));
  const inner = outer - stroke;

  for (let y = 0; y < size; y++) {
    for (let x = 0; x < size; x++) {
      const dist = Math.hypot(x - c, y - c);
      // Feather both edges, then take the tighter one: coverage is zero
      // outside the outer edge and inside the hole.
      const outside = Math.min(1, Math.max(0, outer + 0.5 - dist));
      const inside = Math.min(1, Math.max(0, dist - inner + 0.5));
      const coverage = Math.min(outside, inside);
      if (coverage === 0) continue;
      const i = (y * size + x) * 4;
      rgba[i] = r;
      rgba[i + 1] = g;
      rgba[i + 2] = b;
      rgba[i + 3] = Math.round(coverage * 255);
    }
  }
  return rgba;
}

const RUNNING = [0x34, 0xc7, 0x59]; // green
const STOPPED = [0x8e, 0x8e, 0x93]; // grey
const ERROR = [0xff, 0x9f, 0x0a]; // amber

const ICONS = [
  ['icon.png', 128, RUNNING],
  ['tray-running.png', 32, RUNNING],
  ['tray-stopped.png', 32, STOPPED],
  ['tray-error.png', 32, ERROR],
];

mkdirSync(ICON_DIR, { recursive: true });
for (const [name, size, colour] of ICONS) {
  const path = join(ICON_DIR, name);
  writeFileSync(path, encodePng(ring(size, colour), size));
  console.log(`wrote ${path} (${size}x${size})`);
}
