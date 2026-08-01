// Layer 2 (real-browser, real environment, no mocks) behavioral suite for
// kb-export-subgraph-html's chord-diagram export. Every OTHER test for this
// feature (crates/export/src/html_graph.rs's own `mod tests`) asserts on
// substrings of generated HTML/JS *source text* -- none of them load the
// page and drive it, which is exactly how a real regex-literal-corruption
// bug in the inline-script escaper shipped unnoticed, caught only by
// manually running `node --check` (see html_graph.rs's own doc comment).
// Run against BOTH Chromium and Firefox on purpose: that bug, and a second
// hypothesis investigated and disproven while fixing it (Firefox
// rate-limiting history.pushState under file://), were each only
// reproducible in one specific engine -- Chromium-only testing missed a
// real bug once already in this feature's development history.
//
// "Real editor, not mocks" (CLAUDE.md's own testing philosophy) applies
// here as "real browser, not a DOM mock": every assertion below drives an
// actual page load and actual clicks, never a simulated/stubbed DOM.

const { test } = require("node:test");
const assert = require("node:assert");
const puppeteer = require("puppeteer-core");
const path = require("node:path");

const FIXTURE_PATH = "file://" + path.resolve(__dirname, "fixture.html");
// Same topology as fixture.html, generated with a ChordDiagramConfig
// override (hover_growth_factor: 3.0 vs the default 1.6) -- see
// examples/fixture_export.rs's optional second CLI arg. Exists to confirm
// a config override changes REAL runtime hover-growth behavior, not just
// generated source text.
const CUSTOM_CONFIG_FIXTURE_PATH =
  "file://" + path.resolve(__dirname, "fixture-custom-config.html");

// Six nodes, chosen to exercise the exact translation-completeness matrix
// this feature's untranslated/partial-translation fallback signal cares
// about -- see examples/fixture_export.rs for how each is constructed.
// Node ids match the DOM's #graph-caption / detail-title
// text indirectly via the click sequence below, not hardcoded positions,
// so this suite doesn't depend on chord-layout angles.
const UNTRANSLATED_NODE_TITLE = "Untranslated Node";
const TRANSLATED_NODE_TITLE_ES = "Nodo Completamente Traducido";
const GUIDANCE_NODE_TITLE = "Fixture Style Guide";

const ENGINES = [
  {
    name: "chrome",
    launchOpts: {
      executablePath: "/usr/bin/chromium-browser",
      headless: "new",
      args: ["--no-sandbox", "--disable-setuid-sandbox"],
    },
  },
  {
    name: "firefox",
    launchOpts: {
      browser: "firefox",
      executablePath: "/usr/bin/firefox",
      headless: true,
      args: ["-no-remote"],
    },
  },
];

async function withPage(engine, fn, fixturePath = FIXTURE_PATH, viewport = null) {
  const browser = await puppeteer.launch(engine.launchOpts);
  const errors = [];
  try {
    const page = await browser.newPage();
    // Set BEFORE goto so the responsive/sidebar CSS's @media rules are
    // already in effect on first paint, not applied retroactively after
    // load (which would mask a real flash-of-wrong-layout bug).
    if (viewport) await page.setViewport(viewport);
    page.on("pageerror", (e) => errors.push(`pageerror: ${e.message}`));
    page.on("console", (m) => {
      if (m.type() === "error") errors.push(`console.error: ${m.text()}`);
    });
    await page.goto(fixturePath, { waitUntil: "networkidle0" });
    await new Promise((r) => setTimeout(r, 300));
    await fn(page, errors);
  } finally {
    await browser.close();
  }
}

async function clickNextUntilDisabled(page, maxSteps, delayMs) {
  for (let i = 0; i < maxSteps; i++) {
    const disabled = await page.evaluate(
      () => document.getElementById("next-button").disabled,
    );
    if (disabled) return i;
    await page.click("#next-button");
    await new Promise((r) => setTimeout(r, delayMs));
  }
  return maxSteps;
}

for (const engine of ENGINES) {
  test(`[${engine.name}] toggle language, walk every node both ways, zero uncaught errors`, async () => {
    await withPage(engine, async (page, errors) => {
      // Walk in English first (baseline), then in Spanish -- this is the
      // exact scenario that would have caught the pushState-throttling
      // hypothesis and the fallback-notice bug immediately, months before
      // either was found by hand.
      await page.click("#home-button");
      await new Promise((r) => setTimeout(r, 150));
      await clickNextUntilDisabled(page, 10, 150);

      await page.click("#home-button");
      await new Promise((r) => setTimeout(r, 150));
      await page.click("#lang-toggle");
      await new Promise((r) => setTimeout(r, 200));
      await clickNextUntilDisabled(page, 10, 150);

      assert.deepStrictEqual(
        errors,
        [],
        `expected zero uncaught page errors walking every node in both languages, got: ${JSON.stringify(errors, null, 2)}`,
      );
    });
  });

  test(`[${engine.name}] untranslated node shows the fallback notice; translated node doesn't`, async () => {
    await withPage(engine, async (page) => {
      await page.click("#lang-toggle");
      await new Promise((r) => setTimeout(r, 200));

      // Walk to the untranslated node specifically, by title, rather than
      // assuming a fixed click count -- robust to fixture node-count/order
      // changes.
      let found = false;
      for (let i = 0; i < 10; i++) {
        const title = await page.evaluate(
          () => document.querySelector(".detail-title")?.textContent,
        );
        if (title === UNTRANSLATED_NODE_TITLE) {
          found = true;
          break;
        }
        const disabled = await page.evaluate(
          () => document.getElementById("next-button").disabled,
        );
        if (disabled) break;
        await page.click("#next-button");
        await new Promise((r) => setTimeout(r, 150));
      }
      assert.ok(found, "expected to reach the untranslated fixture node");

      const noticeOnUntranslated = await page.evaluate(
        () => document.querySelector(".translation-fallback-note")?.textContent,
      );
      assert.ok(
        noticeOnUntranslated && noticeOnUntranslated.includes("isn't translated"),
        `expected a fallback notice on the untranslated node, got: ${noticeOnUntranslated}`,
      );

      // Now the translated node: real Spanish content, no notice at all.
      // Reading order ties (equal BFS distance and degree among the five
      // spokes) break alphabetically by id, not fixture-declaration order
      // -- search by title like the untranslated case above rather than
      // assuming a fixed click count lands on "translated" specifically.
      await page.click("#home-button");
      await new Promise((r) => setTimeout(r, 150));
      let translatedTitle = null;
      for (let i = 0; i < 10; i++) {
        const title = await page.evaluate(
          () => document.querySelector(".detail-title")?.textContent,
        );
        if (title === TRANSLATED_NODE_TITLE_ES) {
          translatedTitle = title;
          break;
        }
        const disabled = await page.evaluate(
          () => document.getElementById("next-button").disabled,
        );
        if (disabled) break;
        await page.click("#next-button");
        await new Promise((r) => setTimeout(r, 150));
      }
      assert.strictEqual(translatedTitle, TRANSLATED_NODE_TITLE_ES);
      const noticeOnTranslated = await page.evaluate(
        () => document.querySelector(".translation-fallback-note"),
      );
      assert.strictEqual(
        noticeOnTranslated,
        null,
        "a genuinely translated node must never show the fallback notice",
      );
    });
  });

  test(`[${engine.name}] rapid repeated Next/Previous/toggle clicks stay consistent`, async () => {
    await withPage(engine, async (page, errors) => {
      await page.click("#lang-toggle");
      await new Promise((r) => setTimeout(r, 100));
      // Fire clicks back-to-back with minimal delay -- the exact shape of
      // interaction that originally surfaced this bug in real use.
      for (let i = 0; i < 6; i++) {
        await page.click("#next-button");
        await new Promise((r) => setTimeout(r, 40));
      }
      for (let i = 0; i < 3; i++) {
        await page.click("#prev-button");
        await new Promise((r) => setTimeout(r, 40));
      }
      await page.click("#lang-toggle");
      await new Promise((r) => setTimeout(r, 200));

      const langBtnText = await page.evaluate(
        () => document.getElementById("lang-toggle").textContent,
      );
      const visibleTitle = await page.evaluate(
        () => document.querySelector(".detail-title")?.textContent,
      );
      // After an even number of toggles (2), currentLang is back to "en" --
      // the button label and the visible title must agree on that, not
      // drift apart the way the original bug's stale-UI-state symptom did.
      assert.ok(
        langBtnText.startsWith("EN / ES"),
        `expected the toggle label to reflect currentLang=en after 2 toggles, got: ${langBtnText}`,
      );
      assert.ok(visibleTitle && visibleTitle.length > 0, "expected real visible content, not a blank panel");

      // Back/Forward must still work after this rapid-click sequence.
      await page.goBack();
      await new Promise((r) => setTimeout(r, 200));
      const afterBack = await page.evaluate(
        () => document.querySelector(".detail-title")?.textContent,
      );
      assert.notStrictEqual(
        afterBack,
        visibleTitle,
        "expected Back to actually navigate somewhere different after a rapid-click sequence",
      );
      assert.deepStrictEqual(errors, [], `expected zero uncaught errors during rapid clicking, got: ${JSON.stringify(errors, null, 2)}`);
    });
  });

  test(`[${engine.name}] colophon: guidance node opens via its link, is excluded from the graph and the reading-order walk`, async () => {
    await withPage(engine, async (page, errors) => {
      // The colophon footer exists at all, and is a real DOM element
      // outside #main-content/#sidebar -- deliberately separate from the
      // curated topic content itself.
      const colophonExists = await page.evaluate(
        () => !!document.getElementById("colophon"),
      );
      assert.ok(colophonExists, "expected a #colophon footer in the fixture (it has one guidance node)");

      // Clicking its link opens the guidance node via the SAME real
      // navigation path every other node uses -- detail panel updates,
      // history advances, and the guidance-note orients the reader.
      const clicked = await page.evaluate((title) => {
        const btn = Array.from(document.querySelectorAll(".colophon-link")).find(
          (b) => b.textContent === title,
        );
        if (!btn) return false;
        btn.click();
        return true;
      }, GUIDANCE_NODE_TITLE);
      assert.ok(clicked, "expected a clickable colophon link for the fixture's guidance node");
      await new Promise((r) => setTimeout(r, 200));

      const detailTitle = await page.evaluate(
        () => document.querySelector(".detail-title")?.textContent,
      );
      assert.strictEqual(detailTitle, GUIDANCE_NODE_TITLE);

      const guidanceNote = await page.evaluate(
        () => document.querySelector(".guidance-note")?.textContent,
      );
      assert.ok(
        guidanceNote && guidanceNote.includes("not part of its topic content"),
        `expected a guidance-note orienting the reader, got: ${guidanceNote}`,
      );

      // Excluded from the interactive chord graph: no <g class="node"> for
      // it anywhere in the SVG (draw loop skips is_guidance nodes).
      const drawnInGraph = await page.evaluate(
        () => document.querySelectorAll('#graph-svg [data-kind="practice"]').length,
      );
      assert.strictEqual(drawnInGraph, 0, "the guidance node must not be drawn in the chord graph");

      // Excluded from the Previous/Next reading-order walk: starting fresh
      // from Home and clicking Next through to the end must never land on
      // it.
      await page.click("#home-button");
      await new Promise((r) => setTimeout(r, 150));
      let sawGuidanceInWalk = false;
      for (let i = 0; i < 10; i++) {
        const title = await page.evaluate(
          () => document.querySelector(".detail-title")?.textContent,
        );
        if (title === GUIDANCE_NODE_TITLE) { sawGuidanceInWalk = true; break; }
        const disabled = await page.evaluate(
          () => document.getElementById("next-button").disabled,
        );
        if (disabled) break;
        await page.click("#next-button");
        await new Promise((r) => setTimeout(r, 120));
      }
      assert.ok(
        !sawGuidanceInWalk,
        "the Previous/Next reading-order walk must never land on the guidance node",
      );
      assert.deepStrictEqual(errors, [], `expected zero uncaught errors, got: ${JSON.stringify(errors, null, 2)}`);
    });
  });

  test(`[${engine.name}] an always-throwing history.pushState degrades gracefully`, async () => {
    await withPage(engine, async (page, errors) => {
      // Monkey-patch BEFORE any navigation click -- simulates the real
      // Firefox file:// throttling scenario investigated (and, in this
      // exact form, disproven as the root cause of the reported bug) while
      // fixing the untranslated-node fallback signal, without depending on
      // actually hitting a real browser's rate limit.
      await page.evaluate(() => {
        history.pushState = () => {
          throw new DOMException("simulated: the operation is insecure", "SecurityError");
        };
      });

      const titleBefore = await page.evaluate(
        () => document.querySelector(".detail-title")?.textContent,
      );
      await page.click("#next-button");
      await new Promise((r) => setTimeout(r, 200));
      const titleAfter = await page.evaluate(
        () => document.querySelector(".detail-title")?.textContent,
      );

      assert.notStrictEqual(
        titleAfter,
        titleBefore,
        "content must still update even when history.pushState always throws -- the try/catch guard's whole point",
      );
      assert.deepStrictEqual(
        errors,
        [],
        `expected the simulated pushState throw to be caught, not surfaced as an uncaught page error: ${JSON.stringify(errors, null, 2)}`,
      );

      // Previous/Next's own disabled-state bookkeeping (which sits right
      // after selectNode() in the click handler) must not have been
      // skipped by the thrown-and-uncaught exception this test used to
      // guard against.
      const prevDisabled = await page.evaluate(
        () => document.getElementById("prev-button").disabled,
      );
      assert.strictEqual(
        prevDisabled,
        false,
        "expected Previous to be enabled after a successful Next navigation, even with pushState throwing",
      );
    });
  });

  test(`[${engine.name}] history panel: tracks the visited path, marks Back/Forward neighbors, and truncates stale forward entries on a new path`, async () => {
    await withPage(engine, async (page, errors) => {
      const detailTitle = () => page.evaluate(() => document.querySelector(".detail-title")?.textContent);
      const historyTitles = () => page.evaluate(() =>
        Array.from(document.querySelectorAll("#history-list li")).map((li) => li.textContent),
      );
      const currentText = () => page.evaluate(() => document.querySelector(".history-current")?.textContent);

      await page.click("#home-button");
      await new Promise((r) => setTimeout(r, 150));
      await page.click("#next-button");
      await new Promise((r) => setTimeout(r, 150));
      await page.click("#next-button");
      await new Promise((r) => setTimeout(r, 150));
      // visitStack: [anchor, A, B] -- current is B.
      const titleB = await detailTitle();
      assert.strictEqual(await currentText(), titleB, "the history panel's current entry must match the on-screen node");
      let titles = await historyTitles();
      assert.ok(
        titles.some((t) => t && t.includes("← Back")),
        `expected a "← Back" marker on the entry before current, got: ${JSON.stringify(titles)}`,
      );

      // Native Back: current moves to A, and B (now ahead) gets "Forward →".
      await page.goBack();
      await new Promise((r) => setTimeout(r, 200));
      const titleA = await detailTitle();
      assert.notStrictEqual(titleA, titleB, "expected Back to actually move to a different node");
      assert.strictEqual(await currentText(), titleA);
      titles = await historyTitles();
      assert.ok(
        titles.some((t) => t === titleB + "Forward →" || (t && t.includes(titleB) && t.includes("Forward →"))),
        `expected a "Forward →" marker on ${titleB}, got: ${JSON.stringify(titles)}`,
      );
      const forwardDisabledAtA = await page.evaluate(() => document.getElementById("history-forward").disabled);
      assert.strictEqual(forwardDisabledAtA, false, "expected Forward to be enabled after going Back");

      // Take a genuinely NEW path (Home, not the node Back would have gone
      // forward to) -- the stale forward entry (B) must be truncated, the
      // same way a real browser invalidates forward history on a new
      // navigation, not left as a dangling branch.
      await page.click("#home-button");
      await new Promise((r) => setTimeout(r, 200));
      titles = await historyTitles();
      assert.ok(
        !titles.some((t) => t && t.includes(titleB)),
        `expected ${titleB} to be truncated from history after taking a new path, got: ${JSON.stringify(titles)}`,
      );
      const forwardDisabledAfterNewPath = await page.evaluate(() => document.getElementById("history-forward").disabled);
      assert.strictEqual(
        forwardDisabledAfterNewPath,
        true,
        "expected Forward to be disabled once the stale forward entry was truncated",
      );

      assert.deepStrictEqual(errors, [], `expected zero uncaught errors, got: ${JSON.stringify(errors, null, 2)}`);
    });
  });

  test(`[${engine.name}] history panel: in-panel Back/Forward buttons match native browser navigation, and the depth cap evicts oldest entries visibly`, async () => {
    await withPage(engine, async (page, errors) => {
      const detailTitle = () => page.evaluate(() => document.querySelector(".detail-title")?.textContent);

      await page.click("#home-button");
      await new Promise((r) => setTimeout(r, 150));
      await page.click("#next-button");
      await new Promise((r) => setTimeout(r, 150));
      const titleAfterNext = await detailTitle();

      // #history-back must behave exactly like the native Back button.
      await page.click("#history-back");
      await new Promise((r) => setTimeout(r, 200));
      const titleAfterHistoryBack = await detailTitle();
      assert.notStrictEqual(titleAfterHistoryBack, titleAfterNext, "expected #history-back to actually navigate");

      // #history-forward must behave exactly like native Forward.
      await page.click("#history-forward");
      await new Promise((r) => setTimeout(r, 200));
      const titleAfterHistoryForward = await detailTitle();
      assert.strictEqual(
        titleAfterHistoryForward,
        titleAfterNext,
        "expected #history-forward to return to where #history-back left from",
      );

      // Depth cap: alternate Home/Next repeatedly to push well past the
      // depth cap of 8 distinct navigations (Home and Next each push a new
      // entry as long as the target differs from the current node -- no
      // chord-layout/fixture-node-count dependency needed).
      for (let i = 0; i < 10; i++) {
        await page.click(i % 2 === 0 ? "#next-button" : "#home-button");
        await new Promise((r) => setTimeout(r, 60));
      }
      const truncatedText = await page.evaluate(
        () => document.querySelector(".history-truncated")?.textContent,
      );
      assert.ok(
        truncatedText && /⋯ \d+ earlier/.test(truncatedText),
        `expected a visible "N earlier" truncation indicator once the depth cap was exceeded, got: ${truncatedText}`,
      );
      const entryCount = await page.evaluate(
        () => document.querySelectorAll("#history-list li:not(.history-truncated)").length,
      );
      assert.strictEqual(entryCount, 8, `expected exactly 8 rendered entries at the depth cap, got: ${entryCount}`);

      assert.deepStrictEqual(errors, [], `expected zero uncaught errors, got: ${JSON.stringify(errors, null, 2)}`);
    });
  });

  test(`[${engine.name}] reading order: Next/Previous follow the authored chain (not BFS order), Home lands mid-chain`, async () => {
    await withPage(engine, async (page, errors) => {
      const title = () => page.evaluate(() => document.querySelector(".detail-title")?.textContent);

      // Fresh load: the anchor ("Fixture Home") is auto-selected, and per
      // the fixture's chain (partial-title-only -> home -> untranslated ->
      // translated) it sits at chain position 1, not 0 -- Previous must
      // still work from here (there IS an earlier chain entry).
      assert.strictEqual(await title(), "Fixture Home");
      const prevDisabledAtLoad = await page.evaluate(() => document.getElementById("prev-button").disabled);
      assert.strictEqual(prevDisabledAtLoad, false, "expected Previous to be enabled -- the anchor is not chain position 0");

      await page.click("#prev-button");
      await new Promise((r) => setTimeout(r, 150));
      assert.strictEqual(await title(), "Partial: Title Only", "expected Previous to follow the authored chain to its start");
      const prevDisabledAtStart = await page.evaluate(() => document.getElementById("prev-button").disabled);
      assert.strictEqual(prevDisabledAtStart, true, "expected Previous to be disabled at the real chain start");

      // Forward from the chain start: partial-title-only -> home -> untranslated.
      await page.click("#next-button");
      await new Promise((r) => setTimeout(r, 150));
      assert.strictEqual(await title(), "Fixture Home");
      await page.click("#next-button");
      await new Promise((r) => setTimeout(r, 150));
      assert.strictEqual(await title(), "Untranslated Node", "expected Next to continue along the authored chain, not BFS/alphabetical order");

      // Home jumps back to the anchor's OWN chain position (1), not
      // hardcoded position 0 -- confirmed by Next from Home landing on
      // "Untranslated Node" again (position 2), not "Partial: Title Only"
      // (position 0, which pure `walkIndex = 0` would have produced).
      await page.click("#home-button");
      await new Promise((r) => setTimeout(r, 150));
      assert.strictEqual(await title(), "Fixture Home");
      await page.click("#next-button");
      await new Promise((r) => setTimeout(r, 150));
      assert.strictEqual(await title(), "Untranslated Node", "expected Home to reset walkIndex to the anchor's real chain position, not 0");

      // One more Next reaches "translated" -- the chain's REAL end (its own
      // Reading Order says "Next :: none"). Next must stop there, not
      // spill into the BFS-fallback nodes ("empty-string"/"partial-body-
      // only") that follow it in readingOrder -- a real regression found
      // on gitlab-migration's 167-node export (Next silently wandered into
      // unrelated roadmap content once the authored chain ran out).
      await page.click("#next-button");
      await new Promise((r) => setTimeout(r, 150));
      assert.strictEqual(await title(), "Fully Translated Node", "expected the chain's real end to be reached");
      const nextDisabledAtChainEnd = await page.evaluate(() => document.getElementById("next-button").disabled);
      assert.strictEqual(nextDisabledAtChainEnd, true, "expected Next to stop at the chain's own end, not continue into BFS-fallback content");
      const nextLabelAtChainEnd = await page.evaluate(() => document.getElementById("next-button").textContent);
      assert.ok(nextLabelAtChainEnd.includes("Done"), `expected the Next button to read Done at the chain's end, got: ${nextLabelAtChainEnd}`);

      assert.deepStrictEqual(errors, [], `expected zero uncaught errors, got: ${JSON.stringify(errors, null, 2)}`);
    });
  });

  test(`[${engine.name}] search: a fuzzy (non-substring) query finds and jumps to a node`, async () => {
    await withPage(engine, async (page, errors) => {
      // "Untrnsltd" is a true subsequence match against "Untranslated Node"
      // (vowels/letters dropped) but not a substring -- proves this is
      // real fuzzy matching, not String.includes().
      await page.type("#node-search", "Untrnsltd");
      await new Promise((r) => setTimeout(r, 150));

      const resultTexts = await page.evaluate(() =>
        Array.from(document.querySelectorAll("#search-results button")).map((b) => b.textContent),
      );
      assert.ok(
        resultTexts.includes("Untranslated Node"),
        `expected a fuzzy match for "Untrnsltd", got results: ${JSON.stringify(resultTexts)}`,
      );

      const clicked = await page.evaluate(() => {
        const btn = Array.from(document.querySelectorAll("#search-results button")).find(
          (b) => b.textContent === "Untranslated Node",
        );
        if (!btn) return false;
        btn.click();
        return true;
      });
      assert.ok(clicked, "expected a clickable search result for the fuzzy match");
      await new Promise((r) => setTimeout(r, 200));

      const detailTitle = await page.evaluate(() => document.querySelector(".detail-title")?.textContent);
      assert.strictEqual(detailTitle, "Untranslated Node", "expected clicking a search result to navigate there");

      const resultsHiddenAfterClick = await page.evaluate(() => document.getElementById("search-results").hidden);
      assert.strictEqual(resultsHiddenAfterClick, true, "expected the results dropdown to close after a click");

      assert.deepStrictEqual(errors, [], `expected zero uncaught errors, got: ${JSON.stringify(errors, null, 2)}`);
    });
  });

  test(`[${engine.name}] tag filter: toggling a tag dims non-matching chord nodes; clearing restores them`, async () => {
    await withPage(engine, async (page, errors) => {
      const isFilteredOut = (id) =>
        page.evaluate((nodeId) => {
          const g = document.querySelector('.node[data-id="' + nodeId + '"]');
          return g ? g.classList.contains("filtered-out") : null;
        }, id);

      // Before any filter: nothing is dimmed.
      assert.strictEqual(await isFilteredOut("home"), false);
      assert.strictEqual(await isFilteredOut("untranslated"), false);

      await page.click("#tag-picker-toggle");
      await new Promise((r) => setTimeout(r, 100));
      const pickerVisible = await page.evaluate(() => document.getElementById("tag-picker").hidden === false);
      assert.ok(pickerVisible, "expected the tag picker to open");

      // "i18n" is on translated/untranslated/partial-title-only, NOT on
      // home/partial-body-only/empty-string (see fixture_export.rs).
      const toggled = await page.evaluate(() => {
        const btn = Array.from(document.querySelectorAll("#tag-picker button")).find(
          (b) => b.textContent === "i18n",
        );
        if (!btn) return false;
        btn.click();
        return true;
      });
      assert.ok(toggled, "expected an 'i18n' tag pill in the picker");
      await new Promise((r) => setTimeout(r, 100));

      assert.strictEqual(await isFilteredOut("untranslated"), false, "expected an i18n-tagged node to stay full opacity");
      assert.strictEqual(await isFilteredOut("home"), true, "expected a non-i18n-tagged node to dim once the filter is active");
      // Untagged nodes ("empty-string" has no tags at all) must also dim --
      // OR-within-one-facet semantics means "no active tag" never matches
      // once at least one filter is active.
      assert.strictEqual(await isFilteredOut("empty-string"), true, "expected an untagged node to dim once any tag filter is active");

      const chipText = await page.evaluate(() => document.getElementById("active-tag-chips").textContent);
      assert.ok(chipText.includes("i18n"), `expected an active-filter chip for i18n, got: ${chipText}`);

      // Clear via the chip itself.
      await page.evaluate(() => document.querySelector("#active-tag-chips button").click());
      await new Promise((r) => setTimeout(r, 100));
      assert.strictEqual(await isFilteredOut("home"), false, "expected clearing the filter to restore full opacity");
      assert.strictEqual(await isFilteredOut("empty-string"), false);

      assert.deepStrictEqual(errors, [], `expected zero uncaught errors, got: ${JSON.stringify(errors, null, 2)}`);
    });
  });

  test(`[${engine.name}] neighbor nodes of the selection get a standing highlight, both bigger and distinctly styled`, async () => {
    await withPage(engine, async (page, errors) => {
      const isNeighbor = (id) =>
        page.evaluate((nodeId) => {
          const g = document.querySelector('.node[data-id="' + nodeId + '"]');
          return g ? g.classList.contains("neighbor") : null;
        }, id);
      const circleBox = (id) =>
        page.evaluate((nodeId) => {
          const c = document.querySelector('.node[data-id="' + nodeId + '"] path');
          if (!c) return null;
          const r = c.getBoundingClientRect();
          return r.width * r.height;
        }, id);

      // Fixture edges are a star (every spoke <-> "home" only; no spoke
      // connects directly to another spoke) -- so while a SPOKE is
      // selected, "home" is its one real neighbor and every OTHER spoke is
      // a genuine non-neighbor baseline (while "home" itself is selected,
      // every spoke is a real neighbor, so there's no non-neighbor
      // baseline available at that point -- this is a property of the
      // fixture's topology, not something to work around). Wait out the
      // wedge's own 200ms `d`-attribute transition (STATIC_CSS) before
      // measuring area -- otherwise the geometry check races the CSS
      // animation.
      //
      // Wedges (unlike the old circles) sit at varying distance from the
      // ring center, so their axis-aligned bounding-box area is NOT
      // comparable ACROSS different nodes -- a far spoke's bbox is
      // inherently larger than a near-center anchor's regardless of any
      // growth (confirmed a real false-failure this session: "translated"
      // read as bigger than a *grown* "home" purely from its own position,
      // even with zero growth applied). The valid comparison is the SAME
      // node's own area before vs. after it becomes a neighbor.
      await page.click("#next-button"); // walkIndex 1 (home) -> 2 ("untranslated" per the chain)
      await new Promise((r) => setTimeout(r, 400));
      const selectedAtStep1 = await page.evaluate(() => document.querySelector(".detail-title")?.textContent);
      assert.strictEqual(selectedAtStep1, "Untranslated Node");
      assert.strictEqual(await isNeighbor("home"), true, "expected the selected spoke's real neighbor (home) to be highlighted");
      assert.strictEqual(await isNeighbor("translated"), false, "expected an unrelated spoke to stay unhighlighted");

      const plainArea = await circleBox("translated"); // "translated" is not a neighbor here -- rest-state baseline

      // Selecting "home" makes EVERY spoke a real neighbor, including
      // "translated" -- confirms the highlight isn't stuck from the
      // previous selection, it's genuinely recomputed each time.
      await page.click("#home-button");
      await new Promise((r) => setTimeout(r, 400));
      const neighborArea = await circleBox("translated"); // same node, now a neighbor
      assert.ok(
        neighborArea > plainArea,
        `expected the same node's real on-screen (hit-tested) area to grow once it becomes a highlighted neighbor, got before=${plainArea} after=${neighborArea}`,
      );
      assert.strictEqual(await isNeighbor("translated"), true, "expected home's real neighbors to be highlighted");
      assert.strictEqual(await isNeighbor("home"), false, "expected a node to never highlight itself as its own neighbor");

      // Now select "untranslated" again: "translated" (real neighbor a
      // moment ago, while home was selected) is NOT connected to
      // "untranslated" directly, so its highlight must actually CLEAR, not
      // just accumulate across selections.
      await page.click("#next-button");
      await new Promise((r) => setTimeout(r, 400));
      const selectedAtStep2 = await page.evaluate(() => document.querySelector(".detail-title")?.textContent);
      assert.strictEqual(selectedAtStep2, "Untranslated Node");
      assert.strictEqual(await isNeighbor("translated"), false, "expected the previous neighbor highlight to clear on a new selection");
      assert.strictEqual(await isNeighbor("home"), true, "expected the new selection's real neighbor (home) to be highlighted");

      assert.deepStrictEqual(errors, [], `expected zero uncaught errors, got: ${JSON.stringify(errors, null, 2)}`);
    });
  });

  test(`[${engine.name}] src/example blocks get real syntax highlighting, not plain unstyled text`, async () => {
    await withPage(engine, async (page, errors) => {
      // "Code Sample Node" (examples/fixture_export.rs) has a `#+begin_src
      // tf` block (a keyword, a string, a comment, a number) and a
      // `#+begin_example` block with a "$ terraform plan" prompt line --
      // navigate to it via search, same pattern as the fuzzy-search test.
      await page.type("#node-search", "Code Sample");
      await new Promise((r) => setTimeout(r, 150));
      const clicked = await page.evaluate(() => {
        const btn = Array.from(document.querySelectorAll("#search-results button")).find(
          (b) => b.textContent === "Code Sample Node",
        );
        if (!btn) return false;
        btn.click();
        return true;
      });
      assert.ok(clicked, "expected a clickable search result for Code Sample Node");
      await new Promise((r) => setTimeout(r, 200));

      const detailTitle = await page.evaluate(() => document.querySelector(".detail-title")?.textContent);
      assert.strictEqual(detailTitle, "Code Sample Node");

      // Real assertion, not just class presence: computed color of each
      // token must actually differ from a plain word's color -- proves the
      // CSS rule really applied, not just that the span exists with no
      // visible effect.
      const tokenColorsDiffer = await page.evaluate(() => {
        const body = document.querySelector(".detail-body");
        const kw = body.querySelector(".tok-kw");
        const str = body.querySelector(".tok-str");
        const com = body.querySelector(".tok-com");
        const plain = body.querySelector("pre code"); // the <code> element itself, unstyled default
        if (!kw || !str || !com || !plain) return { ok: false, reason: "missing expected token spans" };
        const kwColor = getComputedStyle(kw).color;
        const strColor = getComputedStyle(str).color;
        const comColor = getComputedStyle(com).color;
        const plainColor = getComputedStyle(plain).color;
        return {
          ok:
            kwColor !== plainColor &&
            strColor !== plainColor &&
            comColor !== plainColor &&
            kwColor !== strColor,
          kwColor,
          strColor,
          comColor,
          plainColor,
        };
      });
      assert.ok(
        tokenColorsDiffer.ok,
        `expected keyword/string/comment tokens to be visibly colored differently from plain code text, got: ${JSON.stringify(tokenColorsDiffer)}`,
      );

      const kwText = await page.evaluate(() => document.querySelector(".detail-body .tok-kw")?.textContent);
      assert.strictEqual(kwText, "resource", "expected the HCL keyword 'resource' to be tagged as a keyword token");

      const promptText = await page.evaluate(() => document.querySelector(".detail-body .tok-prompt")?.textContent);
      assert.strictEqual(promptText, "$", "expected the example block's leading '$ ' to be tagged as a prompt token");

      // The example block's actual command text must survive un-mangled
      // alongside the highlighted "$" -- a regression here would mean the
      // highlighter corrupted real content while decorating it.
      const exampleText = await page.evaluate(() => document.querySelector(".detail-body pre.example")?.textContent);
      assert.ok(
        exampleText && exampleText.includes("terraform plan") && exampleText.includes("No changes."),
        `expected the example block's real text to survive highlighting, got: ${exampleText}`,
      );

      assert.deepStrictEqual(errors, [], `expected zero uncaught errors, got: ${JSON.stringify(errors, null, 2)}`);
    });
  });

  test(`[${engine.name}] history panel stays pinned to the sidebar's bottom edge even when the outline is hidden`, async () => {
    await withPage(engine, async (page, errors) => {
      // "Code Sample Node" has no headings in its body (a plain paragraph
      // plus src/example blocks) -- renderOutline hides #outline-panel
      // entirely for it, which is exactly the case that previously let
      // #history-panel float up to sit right below #graph-pane instead of
      // staying pinned to #sidebar's bottom.
      await page.type("#node-search", "Code Sample");
      await new Promise((r) => setTimeout(r, 150));
      const clicked = await page.evaluate(() => {
        const btn = Array.from(document.querySelectorAll("#search-results button")).find(
          (b) => b.textContent === "Code Sample Node",
        );
        if (!btn) return false;
        btn.click();
        return true;
      });
      assert.ok(clicked, "expected a clickable search result for Code Sample Node");
      await new Promise((r) => setTimeout(r, 200));

      const outlineHidden = await page.evaluate(() => document.getElementById("outline-panel").hidden);
      assert.strictEqual(outlineHidden, true, "expected the outline panel to be hidden for a node with no headings");

      const gap = await page.evaluate(() => {
        const sidebar = document.getElementById("sidebar");
        const historyPanel = document.getElementById("history-panel");
        return sidebar.getBoundingClientRect().bottom - historyPanel.getBoundingClientRect().bottom;
      });
      assert.ok(
        gap < 5,
        `expected #history-panel's bottom edge to sit within 5px of #sidebar's own bottom edge (pinned), got a ${gap}px gap`,
      );

      assert.deepStrictEqual(errors, [], `expected zero uncaught errors, got: ${JSON.stringify(errors, null, 2)}`);
    });
  });

  test(`[${engine.name}] org checkboxes render as real, disabled <input> checkboxes, not literal bracket text`, async () => {
    await withPage(engine, async (page, errors) => {
      // "Code Sample Node" also carries a "- [ ] ..." / "- [X] ..." pair
      // (examples/fixture_export.rs).
      await page.type("#node-search", "Code Sample");
      await new Promise((r) => setTimeout(r, 150));
      const clicked = await page.evaluate(() => {
        const btn = Array.from(document.querySelectorAll("#search-results button")).find(
          (b) => b.textContent === "Code Sample Node",
        );
        if (!btn) return false;
        btn.click();
        return true;
      });
      assert.ok(clicked, "expected a clickable search result for Code Sample Node");
      await new Promise((r) => setTimeout(r, 200));

      const boxes = await page.evaluate(() =>
        Array.from(document.querySelectorAll(".detail-body input[type=\"checkbox\"]")).map((b) => ({
          checked: b.checked,
          disabled: b.disabled,
          text: b.parentElement.textContent.trim(),
        })),
      );
      assert.strictEqual(boxes.length, 2, `expected 2 real checkbox inputs, got: ${JSON.stringify(boxes)}`);
      assert.strictEqual(boxes[0].checked, false);
      assert.strictEqual(boxes[1].checked, true);
      assert.ok(boxes.every((b) => b.disabled), "expected every checkbox to be disabled (read-only export)");
      assert.ok(boxes[0].text.includes("Not done yet"));
      assert.ok(boxes[1].text.includes("Already done"));

      const bodyText = await page.evaluate(() => document.querySelector(".detail-body").textContent);
      assert.ok(!bodyText.includes("[ ]"), "expected no literal '[ ]' bracket text left in the rendered body");
      assert.ok(!bodyText.includes("[X]"), "expected no literal '[X]' bracket text left in the rendered body");

      assert.deepStrictEqual(errors, [], `expected zero uncaught errors, got: ${JSON.stringify(errors, null, 2)}`);
    });
  });

  test(`[${engine.name}] chord nodes render as non-overlapping wedge slices, and hover growth stays anchored (no sideways drift)`, async () => {
    await withPage(engine, async (page, errors) => {
      // Each node group now renders TWO <path>s (the outer wedge + the
      // inner visited-arc band, see html_graph.rs's draw loop) -- exclude
      // the inner arc so this stays a count of real wedge shapes.
      const wedgeCount = await page.evaluate(() => document.querySelectorAll(".node path:not(.visited-inner-arc)").length);
      // Fixture has 7 topic nodes (home + 6 spokes) -- see fixture_export.rs.
      // No `<circle>` should remain from the pre-redesign shape at all.
      assert.strictEqual(wedgeCount, 7, "expected every topic node to render as a wedge <path>, not a <circle>");
      const circleCount = await page.evaluate(() => document.querySelectorAll(".node circle").length);
      assert.strictEqual(circleCount, 0, "expected no leftover non-wedge <circle> node shapes (the visited marker is an inner arc <path> now, not a <circle>)");

      const box = (id) =>
        page.evaluate((nodeId) => {
          const p = document.querySelector('.node[data-id="' + nodeId + '"] path');
          const r = p.getBoundingClientRect();
          return { w: r.width, h: r.height, cx: r.x + r.width / 2, cy: r.y + r.height / 2 };
        }, id);

      const before = await box("translated");
      await page.evaluate(() => {
        document.querySelector('.node[data-id="translated"]').dispatchEvent(
          new MouseEvent("mouseenter", { bubbles: true }),
        );
      });
      await new Promise((r) => setTimeout(r, 400)); // past the 200ms `d` transition
      const after = await box("translated");

      assert.ok(
        after.w * after.h > before.w * before.h,
        `expected hovering a wedge to grow its real on-screen area, got before=${before.w * before.h} after=${after.w * after.h}`,
      );
      // Growth is outward-only, real geometry (not a CSS transform around a
      // possibly-wrong origin -- see the .node path CSS comment for why
      // that approach was tried and reverted this session): the wedge's own
      // bbox CENTER should barely move, even though its bbox literally
      // grows, because growth is symmetric around the ring's own center,
      // not the wedge's local bbox.
      const drift = Math.hypot(after.cx - before.cx, after.cy - before.cy);
      assert.ok(drift < 5, `expected growth to stay anchored (minimal bbox-center drift), got ${drift}px`);

      assert.deepStrictEqual(errors, [], `expected zero uncaught errors, got: ${JSON.stringify(errors, null, 2)}`);
    });
  });

  // Measures the "translated" node wedge's bbox-area growth ratio
  // (after-hover-area / before-hover-area) on whichever fixture is loaded.
  // Reused below to compare the default fixture against one built with a
  // ChordDiagramConfig hover_growth_factor override -- confirms the config
  // changes REAL runtime geometry, not just generated source text (that
  // half is covered Rust-side by hover_growth_factor_override_changes_generated_js).
  async function hoverGrowthAreaRatio(page) {
    const box = (id) =>
      page.evaluate((nodeId) => {
        const p = document.querySelector('.node[data-id="' + nodeId + '"] path');
        const r = p.getBoundingClientRect();
        return { w: r.width, h: r.height };
      }, id);
    const before = await box("translated");
    await page.evaluate(() => {
      document.querySelector('.node[data-id="translated"]').dispatchEvent(
        new MouseEvent("mouseenter", { bubbles: true }),
      );
    });
    await new Promise((r) => setTimeout(r, 400)); // past the 200ms `d` transition
    const after = await box("translated");
    return (after.w * after.h) / (before.w * before.h);
  }

  test(`[${engine.name}] a ChordDiagramConfig hover_growth_factor override produces measurably more hover growth than the default`, async () => {
    let defaultRatio, customRatio;
    const defaultErrors = [];
    const customErrors = [];
    await withPage(engine, async (page, errors) => {
      defaultRatio = await hoverGrowthAreaRatio(page);
      defaultErrors.push(...errors);
    });
    await withPage(
      engine,
      async (page, errors) => {
        customRatio = await hoverGrowthAreaRatio(page);
        customErrors.push(...errors);
      },
      CUSTOM_CONFIG_FIXTURE_PATH,
    );

    assert.ok(
      customRatio > defaultRatio,
      `expected the hover_growth_factor: 3.0 fixture to grow more than the default (1.6) fixture, got default=${defaultRatio} custom=${customRatio}`,
    );
    assert.deepStrictEqual(defaultErrors, [], `expected zero uncaught errors on the default fixture, got: ${JSON.stringify(defaultErrors, null, 2)}`);
    assert.deepStrictEqual(customErrors, [], `expected zero uncaught errors on the custom-config fixture, got: ${JSON.stringify(customErrors, null, 2)}`);
  });

  test(`[${engine.name}] visited-node marker appears after navigating away and persists across further selections`, async () => {
    await withPage(engine, async (page, errors) => {
      const isVisited = (id) =>
        page.evaluate((nodeId) => {
          const g = document.querySelector('.node[data-id="' + nodeId + '"]');
          return g ? g.classList.contains("visited") : null;
        }, id);

      // Load auto-selects the anchor -- selected and visited are mutually
      // exclusive states (see applySelection's own comment), so the anchor
      // shows neither a visited dot nor has any OTHER node visited yet.
      assert.strictEqual(await isVisited("home"), false, "expected the selected anchor to not show its own visited dot");
      assert.strictEqual(await isVisited("untranslated"), false, "expected an unvisited node to have no marker yet");

      await page.click("#next-button"); // home -> untranslated
      await new Promise((r) => setTimeout(r, 300));
      assert.strictEqual(await isVisited("home"), true, "expected home to show a visited marker once navigated away from");
      assert.strictEqual(await isVisited("untranslated"), false, "expected the newly-selected node itself to withhold its own marker");

      await page.click("#next-button"); // untranslated -> next chain node
      await new Promise((r) => setTimeout(r, 300));
      assert.strictEqual(
        await isVisited("untranslated"),
        true,
        "expected the visited marker to persist once navigated away, not just flash momentarily",
      );
      assert.strictEqual(await isVisited("home"), true, "expected an earlier visited marker to survive further navigation");

      assert.deepStrictEqual(errors, [], `expected zero uncaught errors, got: ${JSON.stringify(errors, null, 2)}`);
    });
  });

  test(`[${engine.name}] Part :: breadcrumb appears only for a node with reading-order Part data`, async () => {
    await withPage(engine, async (page, errors) => {
      const breadcrumb = () =>
        page.evaluate(() => {
          const el = document.querySelector(".node-part-breadcrumb");
          return el ? el.textContent : null;
        });

      // "home" (the anchor, initially selected) has a Reading Order section
      // but no Part :: line -- see fixture_export.rs's reading_order_body.
      assert.strictEqual(await breadcrumb(), null, "expected no breadcrumb for a node without a Part :: line");

      await page.click("#next-button"); // home -> untranslated, the one node with Part ::
      await new Promise((r) => setTimeout(r, 300));
      assert.strictEqual(
        await breadcrumb(),
        "Fixture Chain Walkthrough",
        "expected the breadcrumb to show untranslated's authored Part :: label",
      );

      await page.click("#next-button"); // untranslated -> translated, no Part ::
      await new Promise((r) => setTimeout(r, 300));
      assert.strictEqual(await breadcrumb(), null, "expected the breadcrumb to disappear for a node with no Part :: line");

      assert.deepStrictEqual(errors, [], `expected zero uncaught errors, got: ${JSON.stringify(errors, null, 2)}`);
    });
  });

  test(`[${engine.name}] keyboard: ArrowLeft/Right move around the ring, ArrowUp/Down match Next/Previous, Enter activates`, async () => {
    await withPage(engine, async (page, errors) => {
      const activeId = () =>
        page.evaluate(() => {
          const el = document.activeElement;
          return el && el.classList && el.classList.contains("node") ? el.getAttribute("data-id") : null;
        });
      const tabindexOf = (id) =>
        page.evaluate((nodeId) => {
          const g = document.querySelector('.node[data-id="' + nodeId + '"]');
          return g ? g.getAttribute("tabindex") : null;
        }, id);
      const selectedTitle = () => page.evaluate(() => document.querySelector(".detail-title")?.textContent);

      // Only the selected node is ever Tab-reachable (roving tabindex) --
      // focus it directly, the same DOM state a real Tab keypress lands on,
      // since Puppeteer's own Tab-key traversal also has to walk every
      // preceding focusable element in the page first.
      await page.evaluate(() => document.querySelector('.node[data-id="home"]').focus());
      assert.strictEqual(await activeId(), "home");
      assert.strictEqual(await tabindexOf("home"), "0", "expected the selected node to be the sole tabindex=0 stop");

      await page.keyboard.press("ArrowRight");
      await new Promise((r) => setTimeout(r, 350));
      const afterRight = await activeId();
      assert.ok(afterRight && afterRight !== "home", "expected ArrowRight to move focus+selection to an adjacent ring node");
      assert.strictEqual(await tabindexOf("home"), "-1", "expected the previous node to drop out of the tab order");
      assert.strictEqual(await tabindexOf(afterRight), "0", "expected the newly-focused node to become the tab stop");

      await page.keyboard.press("ArrowLeft");
      await new Promise((r) => setTimeout(r, 350));
      assert.strictEqual(await activeId(), "home", "expected ArrowLeft to move back the other way around the ring");

      await page.keyboard.press("ArrowDown"); // reuses the existing Next button/reading-order walk
      await new Promise((r) => setTimeout(r, 350));
      assert.strictEqual(
        await selectedTitle(),
        "Untranslated Node",
        "expected ArrowDown to match the Next button's reading-order behavior",
      );

      await page.keyboard.press("ArrowUp"); // reuses the existing Previous button
      await new Promise((r) => setTimeout(r, 350));
      assert.strictEqual(
        await selectedTitle(),
        "Fixture Home",
        "expected ArrowUp to match the Previous button's reading-order behavior",
      );

      const beforeEnter = await selectedTitle();
      await page.keyboard.press("Enter");
      await new Promise((r) => setTimeout(r, 200));
      assert.strictEqual(
        await selectedTitle(),
        beforeEnter,
        "expected Enter on the already-focused/selected node to be a stable no-op, not an error or a change",
      );

      assert.deepStrictEqual(errors, [], `expected zero uncaught errors, got: ${JSON.stringify(errors, null, 2)}`);
    });
  });

  test(`[${engine.name}] chord diagram fullscreen: expands, navigating a node stays fullscreen, Escape and the toggle both exit`, async () => {
    await withPage(engine, async (page, errors) => {
      const paneState = () =>
        page.evaluate(() => {
          const pane = document.getElementById("graph-pane");
          return {
            isFullscreen: pane.classList.contains("fullscreen"),
            btnLabel: document.getElementById("graph-fullscreen-toggle").getAttribute("aria-label"),
          };
        });

      const before = await paneState();
      assert.strictEqual(before.isFullscreen, false);
      assert.strictEqual(before.btnLabel, "Expand diagram");

      await page.click("#graph-fullscreen-toggle");
      await new Promise((r) => setTimeout(r, 350)); // past the 220ms enter animation
      const entered = await paneState();
      assert.strictEqual(entered.isFullscreen, true, "expected the toggle button to enter fullscreen");
      assert.strictEqual(entered.btnLabel, "Exit fullscreen");

      // Clicking a node while fullscreen navigates but does NOT auto-exit
      // -- the point of the enlarged view is browsing several nodes
      // comfortably, not bouncing back to the small layout on every click.
      const titleBefore = await page.evaluate(() => document.querySelector(".detail-title")?.textContent);
      await page.evaluate(() => {
        document.querySelector('.node[data-id="translated"]').dispatchEvent(
          new MouseEvent("click", { bubbles: true }),
        );
      });
      await new Promise((r) => setTimeout(r, 300));
      const titleAfter = await page.evaluate(() => document.querySelector(".detail-title")?.textContent);
      assert.notStrictEqual(titleAfter, titleBefore, "expected the node click to navigate normally while fullscreen");
      assert.strictEqual(
        (await paneState()).isFullscreen,
        true,
        "expected navigating to a node to leave fullscreen mode untouched",
      );

      await page.keyboard.press("Escape");
      await new Promise((r) => setTimeout(r, 350)); // past the 200ms exit animation
      const afterEscape = await paneState();
      assert.strictEqual(afterEscape.isFullscreen, false, "expected Escape to exit fullscreen");
      assert.strictEqual(afterEscape.btnLabel, "Expand diagram");

      // The same button is also the exit control (not just Escape).
      await page.click("#graph-fullscreen-toggle");
      await new Promise((r) => setTimeout(r, 350));
      await page.click("#graph-fullscreen-toggle");
      await new Promise((r) => setTimeout(r, 350));
      assert.strictEqual(
        (await paneState()).isFullscreen,
        false,
        "expected the toggle button itself to also exit fullscreen",
      );

      assert.deepStrictEqual(errors, [], `expected zero uncaught errors, got: ${JSON.stringify(errors, null, 2)}`);
    });
  });

  test(`[${engine.name}] navigating between nodes resets #main-content's scroll to the top`, async () => {
    await withPage(engine, async (page, errors) => {
      // #main-content is the real scrolling container (overflow-y: auto),
      // not the window -- force a nonzero scroll offset, then confirm a
      // real navigation (an in-body-style click via Next) resets it.
      await page.evaluate(() => { document.getElementById("main-content").scrollTop = 300; });
      const before = await page.evaluate(() => document.getElementById("main-content").scrollTop);
      assert.ok(before > 0, "expected the forced scroll offset to actually take effect before navigating");

      await page.click("#next-button");
      await new Promise((r) => setTimeout(r, 200));
      const after = await page.evaluate(() => document.getElementById("main-content").scrollTop);
      assert.strictEqual(after, 0, "expected navigating to a new node to reset scroll to the top");

      assert.deepStrictEqual(errors, [], `expected zero uncaught errors, got: ${JSON.stringify(errors, null, 2)}`);
    });
  });

  test(`[${engine.name}] the last-open node persists via localStorage and restores on reload`, async () => {
    await withPage(engine, async (page, errors) => {
      await page.click("#next-button"); // home -> untranslated
      await new Promise((r) => setTimeout(r, 200));
      await page.click("#next-button"); // untranslated -> translated
      await new Promise((r) => setTimeout(r, 200));
      const titleBeforeReload = await page.evaluate(() => document.querySelector(".detail-title")?.textContent);
      assert.strictEqual(titleBeforeReload, "Fully Translated Node");

      const stored = await page.evaluate(() => localStorage.getItem("mae-guide-last-node"));
      assert.strictEqual(stored, "translated", "expected the current node to be persisted as the last-open node");

      await page.reload({ waitUntil: "networkidle0" });
      await new Promise((r) => setTimeout(r, 300));
      const titleAfterReload = await page.evaluate(() => document.querySelector(".detail-title")?.textContent);
      assert.strictEqual(
        titleAfterReload,
        "Fully Translated Node",
        "expected reopening the file to resume at the last-open node, not restart at the anchor",
      );

      // Previous/Next must also be resynced to the restored node's real
      // position in the reading order, not stuck at the anchor's -- same
      // resync the Back/Forward popstate handler already does.
      await page.click("#prev-button");
      await new Promise((r) => setTimeout(r, 200));
      const titleAfterPrev = await page.evaluate(() => document.querySelector(".detail-title")?.textContent);
      assert.strictEqual(
        titleAfterPrev,
        "Untranslated Node",
        "expected Previous from the restored node to walk backward from ITS real position, not the anchor's",
      );

      assert.deepStrictEqual(errors, [], `expected zero uncaught errors, got: ${JSON.stringify(errors, null, 2)}`);
    });
  });

  // --- Responsive sidebar / reading-width tests. This is the suite's
  // first use of page.setViewport() -- a CSS-@media-driven feature is
  // exactly what the Rust source-string tests structurally cannot verify
  // (they can only confirm the @media block's TEXT exists), which is the
  // entire reason a Layer 2 real-browser suite exists at all.
  const MOBILE_VIEWPORT = { width: 390, height: 844 };
  const DESKTOP_VIEWPORT = { width: 1440, height: 900 };

  test(`[${engine.name}] mobile: #sidebar starts hidden, the toggle reveals it as a drawer, and the backdrop closes it`, async () => {
    await withPage(engine, async (page, errors) => {
      const sidebarDisplay = () =>
        page.evaluate(() => getComputedStyle(document.getElementById("sidebar")).display);
      const ariaExpanded = () =>
        page.evaluate(() => document.getElementById("sidebar-toggle").getAttribute("aria-expanded"));

      assert.strictEqual(await sidebarDisplay(), "none", "expected the sidebar hidden by default on a phone-width viewport");
      assert.strictEqual(await ariaExpanded(), "false");

      await page.click("#sidebar-toggle");
      await new Promise((r) => setTimeout(r, 350)); // past the 220ms enter animation
      assert.strictEqual(await sidebarDisplay(), "flex", "expected the toggle to reveal the drawer");
      assert.strictEqual(await ariaExpanded(), "true");
      // getBoundingClientRect() must be destructured to plain fields
      // INSIDE the page context -- returning the DOMRect itself across
      // page.evaluate's serialization boundary loses width/height/etc.
      // (they're prototype getters, not own enumerable properties).
      const sidebarWidth = await page.evaluate(() => document.getElementById("sidebar").getBoundingClientRect().width);
      assert.ok(sidebarWidth > 0 && sidebarWidth <= MOBILE_VIEWPORT.width, `expected a right-edge drawer, not a full-viewport overlay, got width=${sidebarWidth}`);

      const backdropDisplay = await page.evaluate(
        () => getComputedStyle(document.getElementById("sidebar-backdrop")).display,
      );
      assert.strictEqual(backdropDisplay, "block", "expected the backdrop to be shown while the drawer is open");

      // Click near the left edge of the viewport, guaranteed to be outside
      // the right-edge drawer's own rect (drawer width <= 320px on a
      // 390px-wide viewport, so x=5 is always clear of it) and therefore
      // land on the exposed backdrop, not the drawer.
      await page.mouse.click(5, 400);
      await new Promise((r) => setTimeout(r, 350)); // past the 200ms exit animation
      assert.strictEqual(await sidebarDisplay(), "none", "expected clicking the backdrop to close the drawer");
      assert.strictEqual(await ariaExpanded(), "false");

      assert.deepStrictEqual(errors, [], `expected zero uncaught errors, got: ${JSON.stringify(errors, null, 2)}`);
    }, FIXTURE_PATH, MOBILE_VIEWPORT);
  });

  test(`[${engine.name}] mobile: opening the chord-diagram fullscreen from inside the open drawer peels back one overlay per Escape`, async () => {
    await withPage(engine, async (page, errors) => {
      await page.click("#sidebar-toggle");
      await new Promise((r) => setTimeout(r, 350));
      await page.click("#graph-fullscreen-toggle");
      await new Promise((r) => setTimeout(r, 350));

      const state = () =>
        page.evaluate(() => ({
          fullscreen: document.getElementById("graph-pane").classList.contains("fullscreen"),
          sidebarOpen: getComputedStyle(document.getElementById("sidebar")).display !== "none",
        }));

      let s = await state();
      assert.strictEqual(s.fullscreen, true, "expected the chord diagram to be fullscreen before pressing Escape");
      assert.strictEqual(s.sidebarOpen, true, "expected the drawer to still be open underneath it");

      await page.keyboard.press("Escape");
      await new Promise((r) => setTimeout(r, 350));
      s = await state();
      assert.strictEqual(s.fullscreen, false, "expected the FIRST Escape to close only the topmost overlay (fullscreen)");
      assert.strictEqual(s.sidebarOpen, true, "expected the drawer to remain open after the first Escape");

      await page.keyboard.press("Escape");
      await new Promise((r) => setTimeout(r, 350));
      s = await state();
      assert.strictEqual(s.sidebarOpen, false, "expected the SECOND Escape to close the drawer");

      assert.deepStrictEqual(errors, [], `expected zero uncaught errors, got: ${JSON.stringify(errors, null, 2)}`);
    }, FIXTURE_PATH, MOBILE_VIEWPORT);
  });

  test(`[${engine.name}] desktop: #detail-panel-content is capped narrower than #main-content and centered`, async () => {
    await withPage(engine, async (page, errors) => {
      await page.click("#next-button"); // home -> untranslated, has real body content to fill the width
      await new Promise((r) => setTimeout(r, 200));

      // Destructure to plain fields inside the page context -- a raw
      // DOMRect returned across page.evaluate loses width/left/right/etc.
      // (prototype getters, not own enumerable properties).
      const rects = await page.evaluate(() => {
        const m = document.getElementById("main-content").getBoundingClientRect();
        const c = document.getElementById("detail-panel-content").getBoundingClientRect();
        return {
          main: { left: m.left, right: m.right, width: m.width },
          content: { left: c.left, right: c.right, width: c.width },
        };
      });

      assert.ok(
        rects.content.width < rects.main.width,
        `expected the 70ch cap to engage at ${DESKTOP_VIEWPORT.width}px (main=${rects.main.width}, content=${rects.content.width})`,
      );
      const leftGap = rects.content.left - rects.main.left;
      const rightGap = rects.main.right - rects.content.right;
      assert.ok(
        Math.abs(leftGap - rightGap) <= 2,
        `expected roughly equal left/right gaps (centered), got left=${leftGap} right=${rightGap}`,
      );

      assert.deepStrictEqual(errors, [], `expected zero uncaught errors, got: ${JSON.stringify(errors, null, 2)}`);
    }, FIXTURE_PATH, DESKTOP_VIEWPORT);
  });

  test(`[${engine.name}] desktop: the sidebar collapse toggle reclaims width and persists across a reload`, async () => {
    await withPage(engine, async (page, errors) => {
      const mainWidthBefore = await page.evaluate(
        () => document.getElementById("main-content").getBoundingClientRect().width,
      );

      await page.click("#sidebar-toggle");
      await new Promise((r) => setTimeout(r, 100)); // desktop collapse is instant, no animation

      const sidebarDisplay = await page.evaluate(() => getComputedStyle(document.getElementById("sidebar")).display);
      assert.strictEqual(sidebarDisplay, "none", "expected the desktop collapse to be instant (no drawer animation)");
      const mainWidthAfter = await page.evaluate(
        () => document.getElementById("main-content").getBoundingClientRect().width,
      );
      assert.ok(mainWidthAfter > mainWidthBefore, "expected #main-content to reclaim the sidebar's width");

      const stored = await page.evaluate(() => localStorage.getItem("mae-guide-sidebar-collapsed"));
      assert.strictEqual(stored, "true");

      await page.reload({ waitUntil: "networkidle0" });
      await new Promise((r) => setTimeout(r, 300));
      const sidebarDisplayAfterReload = await page.evaluate(
        () => getComputedStyle(document.getElementById("sidebar")).display,
      );
      assert.strictEqual(sidebarDisplayAfterReload, "none", "expected the collapsed preference to survive a reload with no extra click");

      assert.deepStrictEqual(errors, [], `expected zero uncaught errors, got: ${JSON.stringify(errors, null, 2)}`);
    }, FIXTURE_PATH, DESKTOP_VIEWPORT);
  });

  test(`[${engine.name}] an explicit stored sidebar preference wins over the new viewport's own default across a resize`, async () => {
    await withPage(engine, async (page, errors) => {
      // Force a collapsed baseline on desktop (stores "closed").
      await page.click("#sidebar-toggle");
      await new Promise((r) => setTimeout(r, 100));

      // Resize to mobile width -- still governed by the stored "closed"
      // value, which happens to match mobile's own default too (a weak
      // signal on its own, hence the next step).
      await page.setViewport(MOBILE_VIEWPORT);
      await new Promise((r) => setTimeout(r, 100));

      // Explicitly re-open the drawer WHILE at mobile width (stores "false").
      await page.click("#sidebar-toggle");
      await new Promise((r) => setTimeout(r, 350));
      assert.strictEqual(
        await page.evaluate(() => getComputedStyle(document.getElementById("sidebar")).display),
        "flex",
        "expected the drawer to actually open at mobile width",
      );

      // Resize back to desktop. Desktop's OWN default is "open" too, so
      // this alone wouldn't isolate "stored preference wins" from
      // "coincidentally matches this breakpoint's default" -- but having
      // forced a COLLAPSED baseline on desktop first (above) means the
      // only way #sidebar is visible here is the stored "false" value
      // actually overriding what would otherwise still be a
      // collapsed-on-desktop state.
      await page.setViewport(DESKTOP_VIEWPORT);
      await new Promise((r) => setTimeout(r, 100));
      const finalDisplay = await page.evaluate(() => getComputedStyle(document.getElementById("sidebar")).display);
      assert.strictEqual(
        finalDisplay,
        "flex",
        "expected the explicit stored preference (opened at mobile width) to persist back across the resize to desktop",
      );

      assert.deepStrictEqual(errors, [], `expected zero uncaught errors, got: ${JSON.stringify(errors, null, 2)}`);
    }, FIXTURE_PATH, DESKTOP_VIEWPORT);
  });
}
