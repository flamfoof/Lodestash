/**
 * Minimal WebGL2 model viewer: orbit, zoom, and one albedo texture per material slot.
 *
 * Hand-rolled rather than three.js on purpose (TECH-STACK §1): the requirement is one mesh, one
 * light, orbit + zoom and a single texture, which is ~300 lines here versus ~150 KB gzipped and a
 * fifth runtime dependency there. `LSM1` already hands us flat `f32`/`u32` arrays, so vertex data
 * goes straight into buffers with no conversion step.
 *
 * Shading matches `lodestash_ingest::render3d` — Lambert over a flat base colour, albedo sampled
 * when present — so the viewer and the thumbnails agree about what a model looks like.
 *
 * Camera behaviour mirrors the egui viewport (`viewport3d.rs:190-205`) so both frontends feel the
 * same: 0.01 rad per pixel dragged, pitch clamped to ±1.54, exponential scroll zoom clamped to
 * 0.05–40× the model radius.
 */
import { parseLsm1, bounds, type Lsm1Mesh } from "./lsm1";

const DRAG_RAD_PER_PX = 0.01;
const PITCH_LIMIT = 1.54;
const ZOOM_PER_NOTCH = 0.002;
const DIST_MIN_FACTOR = 0.05;
const DIST_MAX_FACTOR = 40;

const VERT = `#version 300 es
in vec3 a_pos;
in vec3 a_normal;
in vec2 a_uv;
uniform mat4 u_mvp;
uniform mat4 u_model;
out vec3 v_normal;
out vec2 v_uv;
void main() {
  v_normal = mat3(u_model) * a_normal;
  v_uv = a_uv;
  gl_Position = u_mvp * vec4(a_pos, 1.0);
}`;

const FRAG = `#version 300 es
precision mediump float;
in vec3 v_normal;
in vec2 v_uv;
uniform vec4 u_base;
uniform sampler2D u_tex;
uniform bool u_has_tex;
out vec4 outColor;
void main() {
  vec3 n = normalize(v_normal);
  // Key light over the shoulder plus a fill floor, matching render3d's Lambert term: enough to
  // read form without the unlit half going pure black.
  float lambert = max(dot(n, normalize(vec3(0.4, 0.8, 0.6))), 0.0);
  float shade = 0.35 + 0.65 * lambert;
  vec4 base = u_has_tex ? texture(u_tex, v_uv) : u_base;
  outColor = vec4(base.rgb * shade, 1.0);
}`;

type Batch = {
  /** Material slot (v1 plan S08), or `null` when the response carried no slot trailer. */
  slot: number | null;
  vao: WebGLVertexArrayObject;
  count: number;
  base: [number, number, number, number];
};

function compile(gl: WebGL2RenderingContext, type: number, src: string): WebGLShader {
  const sh = gl.createShader(type)!;
  gl.shaderSource(sh, src);
  gl.compileShader(sh);
  if (!gl.getShaderParameter(sh, gl.COMPILE_STATUS)) {
    throw new Error(`shader: ${gl.getShaderInfoLog(sh)}`);
  }
  return sh;
}

// ── tiny mat4 helpers (column-major, as GL wants) ────────────────────────────

function perspective(fovY: number, aspect: number, near: number, far: number): Float32Array {
  const f = 1 / Math.tan(fovY / 2);
  const m = new Float32Array(16);
  m[0] = f / aspect;
  m[5] = f;
  m[10] = (far + near) / (near - far);
  m[11] = -1;
  m[14] = (2 * far * near) / (near - far);
  return m;
}

function lookAt(eye: number[], target: number[], up: number[]): Float32Array {
  const z = norm(sub(eye, target));
  const x = norm(cross(up, z));
  const y = cross(z, x);
  const m = new Float32Array(16);
  m[0] = x[0]; m[4] = x[1]; m[8] = x[2];
  m[1] = y[0]; m[5] = y[1]; m[9] = y[2];
  m[2] = z[0]; m[6] = z[1]; m[10] = z[2];
  m[12] = -dot(x, eye);
  m[13] = -dot(y, eye);
  m[14] = -dot(z, eye);
  m[15] = 1;
  return m;
}

function translation(t: number[]): Float32Array {
  const m = new Float32Array(16);
  m[0] = m[5] = m[10] = m[15] = 1;
  m[12] = t[0]; m[13] = t[1]; m[14] = t[2];
  return m;
}

function mul(a: Float32Array, b: Float32Array): Float32Array {
  const o = new Float32Array(16);
  for (let c = 0; c < 4; c++) {
    for (let r = 0; r < 4; r++) {
      o[c * 4 + r] =
        a[r] * b[c * 4] +
        a[4 + r] * b[c * 4 + 1] +
        a[8 + r] * b[c * 4 + 2] +
        a[12 + r] * b[c * 4 + 3];
    }
  }
  return o;
}

const sub = (a: number[], b: number[]) => [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
const dot = (a: number[], b: number[]) => a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
const cross = (a: number[], b: number[]) => [
  a[1] * b[2] - a[2] * b[1],
  a[2] * b[0] - a[0] * b[2],
  a[0] * b[1] - a[1] * b[0],
];
const norm = (v: number[]) => {
  const l = Math.hypot(v[0], v[1], v[2]) || 1;
  return [v[0] / l, v[1] / l, v[2] / l];
};

export class Viewer3D {
  private gl: WebGL2RenderingContext;
  private prog: WebGLProgram;
  private batches: Batch[] = [];
  private tex: WebGLTexture | null = null;
  /** Per-slot albedo (v1 plan S08). A batch with no slot texture falls back to `tex`, then colour. */
  private slotTex = new Map<number, WebGLTexture>();
  private center: [number, number, number] = [0, 0, 0];
  private radius = 1;
  private yaw = 0.9;
  private pitch = 0.45;
  private dist = 3;
  private ro: ResizeObserver;

  constructor(private canvas: HTMLCanvasElement) {
    // `preserveDrawingBuffer` so the e2e suite can read pixels back — without it the buffer is
    // undefined after composite and "did it render?" is unanswerable from a test.
    const gl = canvas.getContext("webgl2", {
      antialias: true,
      preserveDrawingBuffer: true,
    });
    if (!gl) throw new Error("WebGL2 unavailable");
    this.gl = gl;

    const p = gl.createProgram()!;
    gl.attachShader(p, compile(gl, gl.VERTEX_SHADER, VERT));
    gl.attachShader(p, compile(gl, gl.FRAGMENT_SHADER, FRAG));
    gl.linkProgram(p);
    if (!gl.getProgramParameter(p, gl.LINK_STATUS)) {
      throw new Error(`link: ${gl.getProgramInfoLog(p)}`);
    }
    this.prog = p;
    gl.enable(gl.DEPTH_TEST);
    gl.enable(gl.CULL_FACE);

    this.ro = new ResizeObserver(() => this.render());
    this.ro.observe(canvas);
  }

  /** The material slots this model is split into, in first-seen order (v1 plan S08). */
  slots(): number[] {
    const seen: number[] = [];
    for (const b of this.batches) {
      if (b.slot != null && !seen.includes(b.slot)) seen.push(b.slot);
    }
    return seen;
  }

  /** Upload geometry and frame it. Replaces anything previously loaded. */
  load(buf: ArrayBuffer) {
    const meshes = parseLsm1(buf);
    this.disposeBatches();
    for (const m of meshes) this.batches.push(this.upload(m));
    const b = bounds(meshes);
    this.center = b.center;
    this.radius = b.radius;
    // Frame the whole model: a bit outside the bounding sphere at the default 45° FOV.
    this.dist = this.radius * 2.8;
    this.render();
  }

  private upload(m: Lsm1Mesh): Batch {
    const gl = this.gl;
    const vao = gl.createVertexArray()!;
    gl.bindVertexArray(vao);

    const attrib = (name: string, data: Float32Array, size: number) => {
      const loc = gl.getAttribLocation(this.prog, name);
      if (loc < 0) return;
      const b = gl.createBuffer();
      gl.bindBuffer(gl.ARRAY_BUFFER, b);
      gl.bufferData(gl.ARRAY_BUFFER, data, gl.STATIC_DRAW);
      gl.enableVertexAttribArray(loc);
      gl.vertexAttribPointer(loc, size, gl.FLOAT, false, 0, 0);
    };
    attrib("a_pos", m.positions, 3);
    // A mesh with no normals still has to shade: fall back to flat up-facing normals rather than
    // leaving the attribute undefined, which renders black.
    attrib("a_normal", m.normals ?? flatNormals(m.positions.length / 3), 3);
    attrib("a_uv", m.uvs ?? new Float32Array((m.positions.length / 3) * 2), 2);

    const ib = gl.createBuffer();
    gl.bindBuffer(gl.ELEMENT_ARRAY_BUFFER, ib);
    gl.bufferData(gl.ELEMENT_ARRAY_BUFFER, m.indices, gl.STATIC_DRAW);
    gl.bindVertexArray(null);

    return {
      slot: m.slot,
      vao,
      count: m.indices.length,
      base: [m.baseColor[0] / 255, m.baseColor[1] / 255, m.baseColor[2] / 255, 1],
    };
  }

  /**
   * Apply an albedo texture — to one material slot, or to the whole model when `slot` is omitted
   * (a material's shader ball, which is one surface by definition). Safe to call before or after
   * `load`: a slot with no geometry yet simply has nothing to draw with it.
   */
  setTexture(img: TexImageSource, slot?: number) {
    const gl = this.gl;
    let tex = slot == null ? this.tex : this.slotTex.get(slot);
    if (!tex) {
      tex = gl.createTexture();
      if (slot == null) this.tex = tex;
      else this.slotTex.set(slot, tex);
    }
    gl.bindTexture(gl.TEXTURE_2D, tex);
    // Unity/FBX UVs have V pointing up; GL samples from the bottom-left.
    gl.pixelStorei(gl.UNPACK_FLIP_Y_WEBGL, true);
    gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, gl.RGBA, gl.UNSIGNED_BYTE, img);
    // **Nearest, not linear** — matching `render3d`'s "nearest-neighbour albedo lookup". These are
    // overwhelmingly palette atlases: a small texture of flat colour blocks that every triangle
    // samples one cell of. Linear filtering blends across cell boundaries and the model comes out
    // speckled with colours that are in neither cell. Mipmaps make it worse for the same reason.
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.NEAREST);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.NEAREST);
    // Atlas-based models (the common case here) tile; wrapping is the safe default.
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.REPEAT);
    gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.REPEAT);
    this.render();
  }

  orbit(dx: number, dy: number) {
    this.yaw += dx * DRAG_RAD_PER_PX;
    this.pitch = Math.max(-PITCH_LIMIT, Math.min(PITCH_LIMIT, this.pitch + dy * DRAG_RAD_PER_PX));
    this.render();
  }

  zoom(scrollDelta: number) {
    const next = this.dist * Math.exp(-scrollDelta * ZOOM_PER_NOTCH);
    this.dist = Math.max(
      this.radius * DIST_MIN_FACTOR,
      Math.min(this.radius * DIST_MAX_FACTOR, next),
    );
    this.render();
  }

  /** Render on change rather than every frame — a still model should not spin the GPU. */
  render() {
    const gl = this.gl;
    const dpr = Math.min(window.devicePixelRatio || 1, 2);
    const w = Math.max(1, Math.round(this.canvas.clientWidth * dpr));
    const h = Math.max(1, Math.round(this.canvas.clientHeight * dpr));
    if (this.canvas.width !== w || this.canvas.height !== h) {
      this.canvas.width = w;
      this.canvas.height = h;
    }
    gl.viewport(0, 0, w, h);
    gl.clearColor(0.055, 0.067, 0.09, 1); // --ls-bg
    gl.clear(gl.COLOR_BUFFER_BIT | gl.DEPTH_BUFFER_BIT);
    if (!this.batches.length) return;

    const cp = Math.cos(this.pitch);
    const eye = [
      this.dist * cp * Math.sin(this.yaw),
      this.dist * Math.sin(this.pitch),
      this.dist * cp * Math.cos(this.yaw),
    ];
    const view = lookAt(eye, [0, 0, 0], [0, 1, 0]);
    const proj = perspective(Math.PI / 4, w / h, this.radius * 0.01, this.radius * 100);
    // Model matrix recentres the mesh on the origin so orbit pivots about the model, not the
    // world — meshes arrive in world space and are frequently far from 0,0,0.
    const model = translation([-this.center[0], -this.center[1], -this.center[2]]);
    const mvp = mul(proj, mul(view, model));

    gl.useProgram(this.prog);
    gl.uniformMatrix4fv(gl.getUniformLocation(this.prog, "u_mvp"), false, mvp);
    gl.uniformMatrix4fv(gl.getUniformLocation(this.prog, "u_model"), false, model);
    gl.activeTexture(gl.TEXTURE0);
    gl.uniform1i(gl.getUniformLocation(this.prog, "u_tex"), 0);
    for (const b of this.batches) {
      // This slot's texture, else the model-wide one, else the flat colour — the same order the
      // renderer resolves (v1 plan S08), so an unpainted slot stays its own colour rather than
      // borrowing a neighbour's atlas.
      const tex = (b.slot != null ? this.slotTex.get(b.slot) : null) ?? this.tex;
      gl.uniform1i(gl.getUniformLocation(this.prog, "u_has_tex"), tex ? 1 : 0);
      if (tex) gl.bindTexture(gl.TEXTURE_2D, tex);
      gl.uniform4fv(gl.getUniformLocation(this.prog, "u_base"), b.base);
      gl.bindVertexArray(b.vao);
      gl.drawElements(gl.TRIANGLES, b.count, gl.UNSIGNED_INT, 0);
    }
    gl.bindVertexArray(null);
  }

  private disposeBatches() {
    for (const b of this.batches) this.gl.deleteVertexArray(b.vao);
    this.batches = [];
  }

  dispose() {
    this.ro.disconnect();
    this.disposeBatches();
    if (this.tex) this.gl.deleteTexture(this.tex);
    for (const t of this.slotTex.values()) this.gl.deleteTexture(t);
    this.slotTex.clear();
    this.gl.deleteProgram(this.prog);
  }
}

function flatNormals(vertexCount: number): Float32Array {
  const n = new Float32Array(vertexCount * 3);
  for (let i = 0; i < vertexCount; i++) n[i * 3 + 1] = 1;
  return n;
}
