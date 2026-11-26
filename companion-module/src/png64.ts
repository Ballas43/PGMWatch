import { deflateSync } from "zlib";

const CRC_TABLE = new Uint32Array(256);
for (let n = 0; n < 256; n++) {
  let c = n;
  for (let k = 0; k < 8; k++) {
    c = (c & 1) !== 0 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
  }
  CRC_TABLE[n] = c >>> 0;
}

function crc32(buf: Buffer): number {
  let c = 0xffffffff;
  for (let i = 0; i < buf.length; i++) {
    c = CRC_TABLE[(c ^ buf[i]) & 0xff] ^ (c >>> 8);
  }
  return (c ^ 0xffffffff) >>> 0;
}

function pngChunk(type: string, data: Buffer): Buffer {
  const len = Buffer.alloc(4);
  len.writeUInt32BE(data.length, 0);
  const typeBuf = Buffer.from(type, "ascii");
  const crcBuf = Buffer.alloc(4);
  crcBuf.writeUInt32BE(crc32(Buffer.concat([typeBuf, data])), 0);
  return Buffer.concat([len, typeBuf, data, crcBuf]);
}

/**
 * Generates a crisp 72x72 RGBA PNG encoded as base64 (`png64`) showing live stereo VU + LUFS bars
 * and tally border color for Bitfocus Companion / Stream Deck keys.
 */
export function renderChannelVuPng64(
  leftDbfs: number,
  rightDbfs: number,
  tallyRgb: [number, number, number]
): string {
  const w = 72;
  const h = 72;
  const raw = Buffer.alloc((w * 4 + 1) * h);

  const normL = Math.max(0, Math.min(1, (leftDbfs + 60) / 60));
  const normR = Math.max(0, Math.min(1, (rightDbfs + 60) / 60));

  for (let y = 0; y < h; y++) {
    const rowStart = y * (w * 4 + 1);
    raw[rowStart] = 0; // filter none
    for (let x = 0; x < w; x++) {
      const px = rowStart + 1 + x * 4;
      let r = 20;
      let g = 20;
      let b = 21;

      const isBorder = x < 3 || x >= w - 3 || y < 3 || y >= h - 3;
      if (isBorder) {
        [r, g, b] = tallyRgb;
      } else if (y >= 16 && y <= 62) {
        const levelFromBottom = (62 - y) / 46;
        const inLeft = x >= 22 && x <= 31;
        const inRight = x >= 40 && x <= 49;
        if ((inLeft && levelFromBottom <= normL) || (inRight && levelFromBottom <= normR)) {
          if (levelFromBottom > 0.88) {
            r = 229;
            g = 72;
            b = 77;
          } else if (levelFromBottom > 0.72) {
            r = 229;
            g = 169;
            b = 59;
          } else {
            r = 76;
            g = 195;
            b = 138;
          }
        } else if (inLeft || inRight) {
          r = 35;
          g = 35;
          b = 37;
        }
      }

      raw[px] = r;
      raw[px + 1] = g;
      raw[px + 2] = b;
      raw[px + 3] = 255;
    }
  }

  const ihdr = Buffer.alloc(13);
  ihdr.writeUInt32BE(w, 0);
  ihdr.writeUInt32BE(h, 4);
  ihdr[8] = 8; // bit depth
  ihdr[9] = 6; // RGBA
  ihdr[10] = 0;
  ihdr[11] = 0;
  ihdr[12] = 0;

  const idat = deflateSync(raw);
  const png = Buffer.concat([
    Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]),
    pngChunk("IHDR", ihdr),
    pngChunk("IDAT", idat),
    pngChunk("IEND", Buffer.alloc(0)),
  ]);
  return png.toString("base64");
}
