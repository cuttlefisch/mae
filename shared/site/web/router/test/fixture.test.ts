// SPDX-License-Identifier: GPL-3.0-or-later
// The palette menu's golden fixture (ADR-119 A2): one scenario per locale
// (scenario.json, scenario-<locale>.json), each with the daemon's page in that
// locale (an input the daemon's own test pins to its shell), the stored
// choice and the steps, and the expected normalised accessibility tree in
// each state. The browser client runs the same files
// against its tests; this runs them against MAE's behaviour.
import { readdirSync, readFileSync, writeFileSync } from "node:fs";
import path from "node:path";
import { Window as HappyWindow } from "happy-dom";
import { describe, expect, it } from "vitest";
import { context, themeControls } from "../src/router";
import { treeOf } from "./a11y";

const DIR = path.join(
  import.meta.dirname,
  "..",
  "..",
  "..",
  "..",
  "..",
  "docs",
  "verification",
  "fixtures",
  "components",
  "palette-menu",
);
const read = (f: string) => readFileSync(path.join(DIR, f), "utf8");
const PREPAINT = readFileSync(path.join(import.meta.dirname, "..", "dist", "prepaint.js"), "utf8");

interface Step {
  focus?: string;
  key?: string;
}
interface Scenario {
  input: string;
  locale: string;
  prefersColorScheme: "light" | "dark";
  storage: Record<string, string>;
  scope: string;
  states: Array<{ name: string; steps: Step[]; expected: string }>;
}
const scenarios: Array<[string, Scenario]> = readdirSync(DIR)
  .filter((f) => /^scenario(-[A-Za-z0-9-]+)?\.json$/.test(f))
  .sort()
  .map((f) => [f, JSON.parse(read(f)) as Scenario]);

/** The page after the pre-paint script and the router's behaviour, then the steps. */
function run(scenario: Scenario, steps: Step[]): Document {
  const win = new HappyWindow({
    url: "https://site.example/kb/parity-fixture/fixture",
    settings: { disableJavaScriptFileLoading: true, disableCSSFileLoading: true, handleDisabledFileLoadingAsSuccess: true },
  });
  win.document.write(read(scenario.input));
  const dom = win as unknown as Window & typeof globalThis;
  const dark = scenario.prefersColorScheme === "dark";
  Object.defineProperty(win, "matchMedia", {
    value: () => ({ matches: dark, addEventListener: () => undefined }),
    configurable: true,
  });
  for (const [k, v] of Object.entries(scenario.storage)) win.localStorage.setItem(k, v);
  new Function("window", PREPAINT)(win);
  themeControls(dom).init(context(dom.document, new AbortController().signal));
  for (const s of steps) {
    if (s.focus) (win.document.querySelector(s.focus) as unknown as HTMLElement).focus();
    if (s.key) {
      const t = win.document.activeElement ?? win.document.body;
      const ev = new win.KeyboardEvent("keydown", { key: s.key, bubbles: true, cancelable: true });
      t.dispatchEvent(ev);
      // A browser activates a focused button on Enter (and Space): a click.
      if (!ev.defaultPrevented && t.tagName === "BUTTON" && (s.key === "Enter" || s.key === " ")) {
        t.dispatchEvent(new win.MouseEvent("click", { bubbles: true, cancelable: true }));
      }
    }
  }
  return win.document as unknown as Document;
}

it("finds a scenario for each locale the fixture covers", () => {
  expect(scenarios.map(([, s]) => s.locale).sort()).toEqual(["en-US", "es-ES", "fr-FR"]);
});

for (const [file, scenario] of scenarios) {
  describe(`the palette menu fixture, ${file} (${scenario.locale})`, () => {
    it("its page is in its locale", () => {
      expect(read(scenario.input)).toContain(`<html lang="${scenario.locale}">`);
    });

    for (const state of scenario.states) {
      it(`${state.name}: the expected accessibility tree`, () => {
        const tree = treeOf(run(scenario, state.steps), scenario.scope);
        // MAE_BLESS_FIXTURES=1 writes the expected tree from the behaviour:
        // the one way to regenerate it, so it is never edited by hand.
        if (process.env.MAE_BLESS_FIXTURES) {
          writeFileSync(path.join(DIR, state.expected), JSON.stringify(tree, null, 2) + "\n");
        }
        expect(tree).toEqual(JSON.parse(read(state.expected)));
      });
    }

    it("the states differ where they must", () => {
      const [closed, open, sub] = scenario.states.map((s) => JSON.parse(read(s.expected)));
      expect(JSON.stringify(closed)).not.toContain("menuitemradio");
      expect(JSON.stringify(open)).toContain('"focused":true');
      expect(JSON.stringify(sub).match(/"role":"menu"/g)).toHaveLength(2);
    });
  });
}
