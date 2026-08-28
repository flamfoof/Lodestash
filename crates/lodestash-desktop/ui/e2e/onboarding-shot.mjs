/**
 * Look at the first-run onboarding panel (UI-SPEC §1).
 *
 *   node e2e/onboarding-shot.mjs [out.png]
 *
 * Not part of the suite — a dev tool, because the suite *cannot* see this screen. `launch()`
 * spends six seconds clicking the panel away so the other 90-odd checks can reach the app, which
 * also means the e2e check written as "if the panel is present, assert things about it" can never
 * fire. A rewritten onboarding screen nearly shipped unseen for exactly that reason.
 *
 * Requires `skip_onboarding` to be false in config — this script does not change it, because
 * flipping a user's settings to take a screenshot is not a thing a screenshot tool should do.
 * Settings › FIRST RUN toggles it, or run with the flag already clear.
 */
import { launch } from "./harness.mjs";

const OUT = process.argv[2] ?? "onboarding.png";
const { page, close } = await launch({ keepOnboarding: true });
try {
  await page.waitForSelector('[data-testid="onboarding"]', { timeout: 10_000 });
  await page.waitForTimeout(900); // the source list and cache detection are async
  await page.screenshot({ path: OUT });
  console.log(
    JSON.stringify(
      await page.evaluate(() => ({
        heading: document.querySelector('[data-testid="onboarding"] h1')?.textContent?.trim(),
        banner: document
          .querySelector('[data-testid="onboarding-existing"]')
          ?.textContent?.replace(/\s+/g, " ")
          .trim(),
        rows: document.querySelectorAll('[data-testid="onboarding-sources"] .src').length,
        buttons: [...document.querySelectorAll('[data-testid="onboarding"] footer button')].map(
          (b) => b.textContent.trim(),
        ),
      })),
      null,
      1,
    ),
  );
  console.log(`\nwrote ${OUT}`);
} catch (e) {
  console.error(
    `no onboarding panel — is skip_onboarding still set? (Settings › FIRST RUN)\n${e.message}`,
  );
  process.exitCode = 1;
} finally {
  await close();
}
