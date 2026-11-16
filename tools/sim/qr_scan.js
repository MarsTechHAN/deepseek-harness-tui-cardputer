#!/usr/bin/env node
// Decode a QR from the PPM that dst would blit to the panel framebuffer.
// Usage: node qr_scan.js file.ppm
const fs = require('fs');
const path = require('path');
let jsQR;
try {
  jsQR = require('jsqr');
} catch (e) {
  console.error('jsqr module missing: run npm install --no-save jsqr in tools/sim');
  process.exit(2);
}
const file = process.argv[2];
if (!file) { console.error('usage: node qr_scan.js file.ppm'); process.exit(2); }
const buf = fs.readFileSync(file);
// P6 header: magic, width, height, maxval (whitespace separated, comments rare)
let pos = 0;
function token() {
  while (pos < buf.length && (buf[pos] === 32 || buf[pos] === 10 || buf[pos] === 13 || buf[pos] === 9)) pos++;
  const start = pos;
  while (pos < buf.length && !(buf[pos] === 32 || buf[pos] === 10 || buf[pos] === 13 || buf[pos] === 9)) pos++;
  return buf.toString('ascii', start, pos);
}
const magic = token();
if (magic !== 'P6') { console.error('not a P6 PPM'); process.exit(2); }
const w = parseInt(token(), 10), h = parseInt(token(), 10), max = parseInt(token(), 10);
while (pos < buf.length && (buf[pos] === 32 || buf[pos] === 10 || buf[pos] === 13 || buf[pos] === 9)) pos++;
if (max !== 255) { console.error('unsupported maxval'); process.exit(2); }
const rgba = new Uint8ClampedArray(w * h * 4);
for (let i = 0, p = pos; i < w * h; i++, p += 3) {
  rgba[i * 4] = buf[p]; rgba[i * 4 + 1] = buf[p + 1]; rgba[i * 4 + 2] = buf[p + 2]; rgba[i * 4 + 3] = 255;
}
const res = jsQR(rgba, w, h, { inversionAttempts: 'dontInvert' });
if (!res) { console.log('DECODE_FAILED'); process.exit(1); }
console.log('DECODED ' + res.data);
