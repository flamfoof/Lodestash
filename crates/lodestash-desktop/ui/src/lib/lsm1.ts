/**
 * `LSM1` mesh parser — the TypeScript half of `lodestash_ingest::mesh::encode_meshes`.
 *
 * That encoder serves double duty: it writes the on-disk mesh cache *and* the `lodestash://mesh` wire
 * format. One encoder, two readers — so this file and `decode_meshes` must stay in lockstep. Drift
 * shows up as garbled geometry rather than an error, which is why the e2e suite parses a real
 * response rather than trusting the shape.
 *
 * Layout, all little-endian:
 *
 *     "LSM1"  u32 mesh_count
 *     per mesh:
 *       u32 name_len, name bytes (utf-8)
 *       u32 n, n*3*f32                positions
 *       u8 has_normals  [u32 n, n*3*f32]
 *       u8 has_uvs      [u32 n, n*2*f32]
 *       u32 idx_count, idx_count*u32  indices
 *       4*u8                          base_color RGBA
 *
 * Optionally followed by the **slot trailer** (v1 plan S08), which the `lodestash://mesh` route
 * appends and the on-disk caches never carry:
 *
 *     "SLT1"  u32 n  n*u32             the material slot of each mesh, in order
 */

export type Lsm1Mesh = {
  name: string;
  positions: Float32Array;
  normals: Float32Array | null;
  uvs: Float32Array | null;
  indices: Uint32Array;
  /** RGBA 0-255, the flat material colour used when there is no albedo texture. */
  baseColor: [number, number, number, number];
  /** Material slot this mesh belongs to (v1 plan S08), or `null` when the source carries no trailer. */
  slot: number | null;
};

const MAGIC = 0x314d534c; // "LSM1" read as a little-endian u32
const SLOT_MAGIC = 0x31544c53; // "SLT1", same reading

/**
 * Parse `LSM1` bytes. Throws on a short or malformed buffer rather than returning a partial mesh —
 * a half-read vertex array renders as garbage, which is far harder to diagnose than a thrown error.
 */
export function parseLsm1(buf: ArrayBuffer): Lsm1Mesh[] {
  const dv = new DataView(buf);
  let o = 0;

  const need = (n: number) => {
    if (o + n > buf.byteLength) {
      throw new Error(`LSM1 truncated: wanted ${n} bytes at ${o} of ${buf.byteLength}`);
    }
  };
  const u32 = () => {
    need(4);
    const v = dv.getUint32(o, true);
    o += 4;
    return v;
  };
  const u8 = () => {
    need(1);
    return dv.getUint8(o++);
  };
  /** Copy `count * stride` floats. A copy, not a view: the source buffer is not aligned. */
  const floats = (count: number, stride: number) => {
    const n = count * stride;
    need(n * 4);
    const out = new Float32Array(n);
    for (let i = 0; i < n; i++) out[i] = dv.getFloat32(o + i * 4, true);
    o += n * 4;
    return out;
  };

  need(4);
  if (dv.getUint32(0, true) !== MAGIC) {
    throw new Error("not an LSM1 mesh (bad magic)");
  }
  o = 4;

  const meshCount = u32();
  const meshes: Lsm1Mesh[] = [];
  for (let m = 0; m < meshCount; m++) {
    const nameLen = u32();
    need(nameLen);
    const name = new TextDecoder().decode(new Uint8Array(buf, o, nameLen));
    o += nameLen;

    const positions = floats(u32(), 3);
    const normals = u8() === 1 ? floats(u32(), 3) : null;
    const uvs = u8() === 1 ? floats(u32(), 2) : null;

    const idxCount = u32();
    need(idxCount * 4);
    const indices = new Uint32Array(idxCount);
    for (let i = 0; i < idxCount; i++) indices[i] = dv.getUint32(o + i * 4, true);
    o += idxCount * 4;

    need(4);
    const baseColor: [number, number, number, number] = [
      dv.getUint8(o),
      dv.getUint8(o + 1),
      dv.getUint8(o + 2),
      dv.getUint8(o + 3),
    ];
    o += 4;

    meshes.push({ name, positions, normals, uvs, indices, baseColor, slot: null });
  }

  // The slot trailer, when the server appended one. Absent in every cache file and in older
  // responses, which is why it is a trailer and not a field: those still parse exactly as before.
  if (o + 8 <= buf.byteLength && dv.getUint32(o, true) === SLOT_MAGIC) {
    o += 4;
    const n = u32();
    if (n === meshes.length && o + n * 4 <= buf.byteLength) {
      for (let i = 0; i < n; i++) meshes[i].slot = dv.getUint32(o + i * 4, true);
    }
  }
  return meshes;
}

/** Axis-aligned bounds over every mesh — what the camera frames on load. */
export function bounds(meshes: Lsm1Mesh[]): { center: [number, number, number]; radius: number } {
  let minX = Infinity,
    minY = Infinity,
    minZ = Infinity;
  let maxX = -Infinity,
    maxY = -Infinity,
    maxZ = -Infinity;
  for (const m of meshes) {
    const p = m.positions;
    for (let i = 0; i < p.length; i += 3) {
      if (p[i] < minX) minX = p[i];
      if (p[i] > maxX) maxX = p[i];
      if (p[i + 1] < minY) minY = p[i + 1];
      if (p[i + 1] > maxY) maxY = p[i + 1];
      if (p[i + 2] < minZ) minZ = p[i + 2];
      if (p[i + 2] > maxZ) maxZ = p[i + 2];
    }
  }
  if (!Number.isFinite(minX)) return { center: [0, 0, 0], radius: 1 };
  const center: [number, number, number] = [
    (minX + maxX) / 2,
    (minY + maxY) / 2,
    (minZ + maxZ) / 2,
  ];
  const radius =
    Math.max(maxX - minX, maxY - minY, maxZ - minZ) / 2 || 1;
  return { center, radius };
}
