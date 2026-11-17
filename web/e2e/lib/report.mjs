// Checks with budgets, and the per-run CSV report (owner's convention, TECH-STACK §5.2):
// `test-reports/MM-DD-HH-MM.csv`, local 24-hour time, one row per check —
// origin, check, status, duration_ms, detail. A hang is a failure: a check gets 5 s unless it
// declares a longer budget at its call site, with the reason beside it.

import fs from "node:fs";
import path from "node:path";

const DEFAULT_BUDGET_MS = 5000;

export function createReport({ origin, root }) {
  const rows = [];

  /**
   * Run `fn` as one check. Never throws: a failure (or an overrun) is recorded and returned as
   * `{ ok: false, detail }`, so the caller carries on and every later check is still reported.
   */
  async function check(name, fn, { budget = DEFAULT_BUDGET_MS } = {}) {
    const started = performance.now();
    let timer;
    let ok = false;
    let detail = "";
    let value;
    try {
      value = await Promise.race([
        fn(),
        new Promise((_, reject) => {
          timer = setTimeout(
            () => reject(new Error(`took longer than ${budget} ms — a check that needs longer must declare its budget`)),
            budget,
          );
        }),
      ]);
      ok = true;
      if (typeof value === "string") detail = value;
    } catch (e) {
      detail = String(e?.message ?? e).split("\n")[0];
    } finally {
      clearTimeout(timer);
    }
    const ms = Math.round(performance.now() - started);
    rows.push({ name, ok, ms, detail });
    console.log(`  ${ok ? "ok  " : "FAIL"} ${name}${detail ? `  — ${detail}` : ""}`);
    return { ok, value, detail };
  }

  /** Record a check that could not run (its prerequisite failed): a failure, never a skip. */
  function notRun(name, why) {
    rows.push({ name, ok: false, ms: 0, detail: `not run: ${why}` });
    console.log(`  FAIL ${name}  — not run: ${why}`);
  }

  function write() {
    const d = new Date();
    const p2 = (n) => String(n).padStart(2, "0");
    const stamp = `${p2(d.getMonth() + 1)}-${p2(d.getDate())}-${p2(d.getHours())}-${p2(d.getMinutes())}`;
    const dir = path.join(root, "test-reports");
    fs.mkdirSync(dir, { recursive: true });
    // Two runs in one minute must not overwrite each other's report.
    let file = path.join(dir, `${stamp}.csv`);
    for (let n = 2; fs.existsSync(file); n++) file = path.join(dir, `${stamp}-${n}.csv`);
    const cell = (v) => (/[",\n]/.test(String(v)) ? `"${String(v).replace(/"/g, '""')}"` : String(v));
    const lines = [
      ["origin", "check", "status", "duration_ms", "detail"],
      ...rows.map((r) => [origin, r.name, r.ok ? "pass" : "fail", r.ms, r.detail]),
    ];
    fs.writeFileSync(file, lines.map((l) => l.map(cell).join(",")).join("\n") + "\n");
    return file;
  }

  return {
    check,
    notRun,
    write,
    get rows() {
      return rows;
    },
    get failures() {
      return rows.filter((r) => !r.ok);
    },
  };
}
