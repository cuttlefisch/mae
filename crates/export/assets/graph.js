(function () {
  "use strict";
  var data = JSON.parse(document.getElementById("graph-data").textContent);
  var nodes = data.nodes;
  var edges = data.edges;
  var anchorId = data.anchorId;
  var hasTranslations = data.hasTranslations;
  // Real data injection from Rust's ChordDiagramConfig, not exact-substring
  // text patching -- defaults here match ChordDiagramConfig::default()
  // exactly, so this file stays independently valid/correct JS (and
  // `node --check`-able) even when loaded standalone with no chordConfig
  // in the payload (e.g. a hand-built fixture in a test).
  var chordConfig = data.chordConfig || {};

  var nodesById = {};
  nodes.forEach(function (n, i) { n._idx = i; nodesById[n.id] = n; });
  // ADR-079: guidance/colophon nodes are real entries in `nodes` (so
  // nodesById/selectNode/renderDetail all resolve them like any other node
  // when opened from the colophon), but never part of the interactive
  // chord graph or the reading-order walk -- topicNodes is what those two
  // things iterate over instead of `nodes` directly.
  var topicNodes = nodes.filter(function (n) { return !n.is_guidance; });

  var svg = document.getElementById("graph-svg");
  var popover = document.getElementById("popover");
  var graphCaption = document.getElementById("graph-caption");
  var mainContent = document.getElementById("main-content");
  var detailContent = document.getElementById("detail-panel-content");
  var outlinePanel = document.getElementById("outline-panel");
  var outlineList = document.getElementById("outline-list");
  var outlineToggle = document.getElementById("outline-toggle");
  var historyList = document.getElementById("history-list");
  var historyBackBtn = document.getElementById("history-back");
  var historyForwardBtn = document.getElementById("history-forward");
  var langToggle = document.getElementById("lang-toggle");
  var nextBtn = document.getElementById("next-button");
  var prevBtn = document.getElementById("prev-button");
  var homeBtn = document.getElementById("home-button");
  var themeToggle = document.getElementById("theme-toggle");
  var nodeSearch = document.getElementById("node-search");
  var searchResults = document.getElementById("search-results");
  var tagPickerToggle = document.getElementById("tag-picker-toggle");
  var tagPicker = document.getElementById("tag-picker");
  var activeTagChips = document.getElementById("active-tag-chips");
  var tagFilterGroup = document.querySelector(".tag-filter-group");
  var graphPane = document.getElementById("graph-pane");
  var fullscreenToggle = document.getElementById("graph-fullscreen-toggle");
  var sidebarEl = document.getElementById("sidebar");
  var sidebarToggle = document.getElementById("sidebar-toggle");
  var sidebarBackdrop = document.getElementById("sidebar-backdrop");
  var pageHeader = document.getElementById("page-header");

  // Keeps the mobile drawer/backdrop (STATIC_CSS: top: var(--header-h))
  // starting below the REAL header, whatever height it's actually
  // rendering at (its own flex-wrap can push it to a second row on a
  // narrow viewport) -- a real bug this fixes: a fixed guess or a
  // z-index-boosted header sitting ABOVE the drawer both put the
  // header's own hit-test region on top of the drawer's top strip,
  // silently swallowing clicks on controls near the sidebar's own top
  // edge (#graph-fullscreen-toggle). Re-synced on every header resize,
  // not just once on load.
  function syncHeaderHeightVar() {
    document.documentElement.style.setProperty("--header-h", pageHeader.getBoundingClientRect().height + "px");
  }
  syncHeaderHeightVar();
  if (window.ResizeObserver) {
    new ResizeObserver(syncHeaderHeightVar).observe(pageHeader);
  } else {
    window.addEventListener("resize", syncHeaderHeightVar);
  }

  var currentLang = "en";
  var selectedId = null;

  // --- Visited-node history panel state -- a shadow copy of what WE have
  // pushed via history.pushState, not a read of the browser's own history
  // (JS has no read access to that). Modeled the same way real session
  // history works: an ordered stack plus a position pointer, not just a
  // running log, so going Back then taking a NEW path correctly drops the
  // old "forward" entries instead of leaving a stale branch visible. ---
  var visitStack = [anchorId];
  var visitPos = 0;
  var visitDropped = 0; // oldest entries HISTORY_DEPTH_CAP has evicted -- rendered as "N earlier", never silently
  var HISTORY_DEPTH_CAP = chordConfig.historyDepthCap ?? 8;
  // Every node ever selected, for the ring's visited-node marker (below,
  // applySelection). Deliberately NOT derived from visitStack at render
  // time: visitStack is capped (HISTORY_DEPTH_CAP) and evicts its oldest
  // entries, but a "visited" mark on the ring should behave like a real
  // visited-link convention -- once seen, stays marked seen, regardless of
  // how much has been visited since.
  var visitedIds = {};
  visitedIds[anchorId] = true;

  if (hasTranslations) {
    langToggle.hidden = false;
  }

  // --- Layout: fit all node positions (chord-ring or force, whichever the
  // export baked in) into the SVG viewBox. Center is used both for the
  // viewBox fit AND as the pull-point for edge arcs below. Node titles no
  // longer render as in-SVG <text> at all (see the node-drawing loop and
  // #graph-caption below) -- they show, at a real legible size, in a
  // caption under the diagram instead.
  var minX = Math.min.apply(null, topicNodes.map(function (n) { return n.x; }));
  var maxX = Math.max.apply(null, topicNodes.map(function (n) { return n.x; }));
  var minY = Math.min.apply(null, topicNodes.map(function (n) { return n.y; }));
  var maxY = Math.max.apply(null, topicNodes.map(function (n) { return n.y; }));
  if (!isFinite(minX)) { minX = 0; maxX = 100; minY = 0; maxY = 100; }
  var w = Math.max(1, maxX - minX);
  var h = Math.max(1, maxY - minY);
  var centerX = (minX + maxX) / 2, centerY = (minY + maxY) / 2;

  function degreeOf(id) {
    var d = 0;
    edges.forEach(function (e) { if (e.source === id || e.target === id) d++; });
    return d;
  }

  // `renderedWidth` is governed by CSS (#graph-svg's fixed ~280-300px box),
  // not by the viewBox we're about to compute -- safe to read before the
  // viewBox is set.
  var renderedWidth = svg.getBoundingClientRect().width || 300;
  var minOnscreenRadiusPx = chordConfig.minOnscreenRadiusPx ?? 12;

  // A wedge's OUTER arc corners sit at its own angle +/- a half-span (see
  // the draw loop below), NOT directly outward from the node's own (x, y).
  // For a node far out on the ring, that angular offset swings the corner
  // sideways by a real WORLD distance (~= outerR * halfSpan) that can
  // dwarf the radial growth itself -- confirmed as a real clipping bug
  // this session: padding sized only to cover "radial reach past the
  // node's own radius" clipped ordinary spoke wedges on hover, NOT the
  // anchor (despite the anchor having the largest thickness bonus),
  // because the anchor sits near center where that angular swing is tiny
  // while a far spoke's is not. Bounding the swing per-angle isn't worth
  // the complexity for a nav widget -- instead, require the viewBox to
  // contain a full circle of radius `maxOuterR` around the ring center.
  // Deliberately conservative (not the tightest possible pad), cheap to
  // compute, and correct no matter where in its span a corner lands.
  var maxNodeRadius = 0;
  topicNodes.forEach(function (n) {
    var r = Math.sqrt((n.x - centerX) * (n.x - centerX) + (n.y - centerY) * (n.y - centerY));
    maxNodeRadius = Math.max(maxNodeRadius, r);
  });
  // refreshWedgeGrowth's hover bonus is `minWorldRadius * (HOVER_GROWTH_FACTOR - 1)`
  // -- i.e. a hovered wedge's total outward reach past its own radius is
  // minWorldRadius * HOVER_GROWTH_FACTOR, not just minWorldRadius. Must
  // match refreshWedgeGrowth's formula exactly or the two drift out of
  // sync (a real bug found this session).
  var HOVER_GROWTH_FACTOR = chordConfig.hoverGrowthFactor ?? 1.6;
  var strokeBuffer = chordConfig.strokeBufferPx ?? 2;
  // Extra flat cushion beyond the strict correctness minimum, purely for
  // visual breathing room around the ring (user-requested) -- doesn't
  // affect centering, since pad is always applied symmetrically (see
  // centerX/centerY above, and viewBoxW below).
  var cosmeticCushion = chordConfig.cosmeticCushionPx ?? 16;

  // pad and minWorldRadius are circularly defined (pad affects viewBoxW,
  // which affects worldToScreenScale, which affects minWorldRadius, which
  // pad must cover) -- two passes converges close enough: pad's effect on
  // scale is second-order once `w`/`h` dominate viewBoxW, true except at
  // very low node counts where minWorldRadius is tiny anyway. Not a
  // precision-critical fit, just a nav widget.
  var pad = chordConfig.initialPadPx ?? 40;
  for (var fitPass = 0; fitPass < 2; fitPass++) {
    var viewBoxWGuess = w + pad * 2;
    var scaleGuess = renderedWidth / viewBoxWGuess;
    var minWorldRadiusGuess = minOnscreenRadiusPx / scaleGuess;
    // Every wedge's base thickness is exactly minWorldRadius (uniform --
    // see the draw loop's halfThickness, and its own comment on why a
    // per-node bonus was dropped), so there's no separate bonus term to
    // budget for here beyond the hover-growth reach.
    var maxOuterR = maxNodeRadius + minWorldRadiusGuess * HOVER_GROWTH_FACTOR + strokeBuffer;
    pad = Math.max(maxOuterR - w / 2, maxOuterR - h / 2, 0) + cosmeticCushion;
  }
  var viewBoxW = w + pad * 2;
  svg.setAttribute(
    "viewBox",
    (minX - pad) + " " + (minY - pad) + " " + viewBoxW + " " + (h + pad * 2)
  );
  svg.setAttribute("preserveAspectRatio", "xMidYMid meet");

  // Final scale/floor, from the FINAL (not provisional) viewBoxW -- see the
  // node-drawing loop below for why a flat world-space floor alone can't
  // guarantee a real on-screen hit target (preserveAspectRatio scales world
  // units down as the ring grows; a sufficient floor at 14 nodes silently
  // stops being sufficient at 50+).
  var worldToScreenScale = renderedWidth / viewBoxW;
  var minWorldRadius = minOnscreenRadiusPx / worldToScreenScale;

  var svgNS = "http://www.w3.org/2000/svg";
  function el(tag, attrs) {
    var n = document.createElementNS(svgNS, tag);
    for (var k in attrs) { n.setAttribute(k, attrs[k]); }
    return n;
  }
  // DOM (not SVG) element helper for the sidebar/detail panel, used
  // everywhere below instead of string-concatenated innerHTML so plain
  // text content only ever goes through textContent, never through
  // string interpolation into markup.
  function dom(tag, attrs, text) {
    var n = document.createElement(tag);
    for (var k in (attrs || {})) { n.setAttribute(k, attrs[k]); }
    if (text != null) { n.textContent = text; }
    return n;
  }

  // --- Draw edges first (under nodes), as arcs pulled toward the layout
  // center -- the chord-diagram convention -- rather than straight chords.
  // Each edge is independently clickable ("follow the link to the other
  // node"): clicking navigates to whichever endpoint ISN'T currently
  // selected (defaulting to the target when neither/both match). ---
  var edgeLayer = el("g", { id: "edge-layer" });
  svg.appendChild(edgeLayer);
  var edgePaths = [];
  edges.forEach(function (e) {
    var s = nodesById[e.source], t = nodesById[e.target];
    if (!s || !t) { return; }
    // ADR-079: a guidance node has no real chord position (x=0, y=0,
    // never drawn as a <g class="node"> below) -- an edge touching one
    // would otherwise draw a visually broken arc toward the coordinate
    // origin. Guidance nodes aren't expected to have topic-graph edges in
    // practice (they're pulled in as always-included meta content, not via
    // normal traversal), but this stays a real filter, not an assumption.
    if (s.is_guidance || t.is_guidance) { return; }
    var pullBack = chordConfig.edgePullBack ?? 0.55; // 0 = straight line, 1 = fully at center
    var cx = s.x + (t.x - s.x) / 2 + (centerX - (s.x + t.x) / 2) * pullBack;
    var cy = s.y + (t.y - s.y) / 2 + (centerY - (s.y + t.y) / 2) * pullBack;
    // Land the vertex on each node's wedge INNER edge, not its raw (x, y)
    // -- which sits at nodeRadius, the wedge's mid-thickness point (every
    // wedge's halfThickness is exactly minWorldRadius; see above) -- so
    // the chord visually meets the slice instead of appearing to
    // originate from somewhere inside it. edgeVertexInset nudges a touch
    // further in so the vertex doesn't sit exactly on the boundary line
    // (avoids an anti-aliasing seam at that exact radius). The curve's
    // control point (cx, cy) above is left keyed to the nodes' own ring
    // positions -- only the endpoints move.
    var edgeVertexInset = 2;
    var sAngle = Math.atan2(s.y - centerY, s.x - centerX);
    var sRadius = Math.sqrt((s.x - centerX) * (s.x - centerX) + (s.y - centerY) * (s.y - centerY));
    var sVertex = polarPoint(centerX, centerY, sRadius - minWorldRadius - edgeVertexInset, sAngle);
    var tAngle = Math.atan2(t.y - centerY, t.x - centerX);
    var tRadius = Math.sqrt((t.x - centerX) * (t.x - centerX) + (t.y - centerY) * (t.y - centerY));
    var tVertex = polarPoint(centerX, centerY, tRadius - minWorldRadius - edgeVertexInset, tAngle);
    var d = "M " + sVertex[0] + " " + sVertex[1] + " Q " + cx + " " + cy + " " + tVertex[0] + " " + tVertex[1];
    var path = el("path", { d: d, class: "edge", "data-source": e.source, "data-target": e.target });
    path.addEventListener("click", function () {
      selectNode(selectedId === e.source ? e.target : e.source);
    });
    edgeLayer.appendChild(path);
    edgePaths.push(path);
  });

  // --- Draw nodes as arc slices (wedges) of the ring instead of
  // overlapping circles -- kb/adrs/00XX. Every topic node sits at the SAME
  // radius from center (chord_ring_positions, upstream in mae-canvas
  // outside this file) and gets an equal angular slot; the radial
  // thickness carries forward the EXACT same size formula circles used
  // (anchor/degree bonus on top of the >=24px on-screen floor), just
  // doubled to express thickness (both sides of the ring) instead of a
  // single-sided radius. The amount a wedge reaches beyond the ring's own
  // radius (thickness/2) is therefore numerically identical to the old
  // circle radius `r` -- exactly what the two-pass viewBox `pad` fit
  // above already budgets for, so that code needed zero changes.
  var ringNodeCount = topicNodes.length;
  var angleStep = ringNodeCount > 0 ? (2 * Math.PI / ringNodeCount) : 0;
  // No angular gap between adjacent wedges (user request) -- slots sit
  // flush against each other; the rounded corners below (cornerRadius)
  // are what visually separate one wedge from the next, the same way
  // adjacent flower petals read as distinct without a drawn gap between
  // them.
  var wedgeGapRadians = chordConfig.wedgeGapRadians ?? 0;

  function polarPoint(cx, cy, r, a) {
    return [cx + r * Math.cos(a), cy + r * Math.sin(a)];
  }

  // Annular-sector `d` path with optionally-rounded corners ("petal"
  // look, user request). a1 is always > a0 (both are angle +/- a
  // half-span). With cornerRadius 0 this is the plain sharp-cornered
  // sector: outer arc a0->a1 (sweep=1, increasing angle), straight in to
  // innerR, inner arc a1->a0 (sweep=0, decreasing angle), close -- the
  // standard annulus-sector construction (the same shape a d3.js-style
  // arc() generator produces).
  //
  // With cornerRadius > 0, each of the 4 corners (where a RADIAL edge
  // meets a CIRCULAR arc -- always a clean 90-degree corner, since a
  // radial line's direction is purely radial and a circle's tangent at
  // any point is purely perpendicular to its radius) is filleted: back
  // off `cr` along each adjoining edge and connect with a small arc of
  // radius `cr`. This is an approximation of the true circle-tangency
  // corner-radius algorithm (treating the arc edges as locally straight
  // right at the corner) -- accurate enough to look properly rounded for
  // any `cr` that's small relative to innerR/outerR, which every caller
  // here respects via the clamp below.
  // innerCornerRadius is OPTIONAL and defaults to outerCornerRadius --
  // every existing call site (the main wedge, both here and in
  // refreshWedgeGrowth) passes a single radius and gets the original
  // symmetric-corner behavior unchanged. The two-radius form exists for
  // visited-inner-arc below: that band's outer edge (innerArcOuterR) is
  // an artificial internal cut partway through the wedge, not a real
  // wedge boundary -- rounding it made the marker look like a separate
  // floating pill instead of a flush-sided slice of the petal it's
  // nested in (a real, reported visual mismatch). Its inner edge
  // (innerR) IS a real, SHARED boundary with the wedge itself, so that
  // corner still wants rounding, matching the wedge's own inner-corner
  // treatment there.
  function arcPath(cx, cy, innerR, outerR, a0, a1, outerCornerRadius, innerCornerRadius) {
    if (innerCornerRadius === undefined) { innerCornerRadius = outerCornerRadius; }
    var crOuter = outerCornerRadius || 0;
    var crInner = innerCornerRadius || 0;
    // Clamp each radius independently so opposite fillets on the SAME
    // edge (outer or inner) can never meet or cross -- otherwise a very
    // thin (small halfThickness) or very short (small angular span)
    // wedge would self-intersect into a degenerate shape instead of just
    // a less-rounded one. Both are also capped by the full radial
    // thickness so one edge's fillet can't bite into the other edge's
    // radius even when the two differ.
    crOuter = Math.max(
      0,
      Math.min(crOuter, (outerR - innerR) / 2 - 0.01, ((a1 - a0) * outerR) / 2 - 0.01)
    );
    crInner = Math.max(
      0,
      Math.min(
        crInner,
        (outerR - innerR) / 2 - 0.01,
        ((a1 - a0) * Math.max(innerR, 1)) / 2 - 0.01
      )
    );
    if (crOuter <= 0 && crInner <= 0) {
      var ox0 = polarPoint(cx, cy, outerR, a0), ox1 = polarPoint(cx, cy, outerR, a1);
      var ix1 = polarPoint(cx, cy, innerR, a1), ix0 = polarPoint(cx, cy, innerR, a0);
      return "M " + ox0[0] + " " + ox0[1] +
        " A " + outerR + " " + outerR + " 0 0 1 " + ox1[0] + " " + ox1[1] +
        " L " + ix1[0] + " " + ix1[1] +
        " A " + innerR + " " + innerR + " 0 0 0 " + ix0[0] + " " + ix0[1] +
        " Z";
    }
    // A zero radius here degenerates to a straight line to the same
    // endpoint the rounded case would have used (SVG's own rule for a
    // zero-radius arc segment) -- crOuter/crInner independently at 0
    // naturally produces a flush, unrounded edge on just that side with
    // no special-casing needed beyond the shared branch above.
    var dOuter = crOuter / outerR;
    var dInner = crInner / Math.max(innerR, 1);
    var pOuterStart = polarPoint(cx, cy, outerR, a0 + dOuter);
    var pOuterEnd = polarPoint(cx, cy, outerR, a1 - dOuter);
    var pSideEndOuter = polarPoint(cx, cy, outerR - crOuter, a1);
    var pSideEndInner = polarPoint(cx, cy, innerR + crInner, a1);
    var pInnerEnd = polarPoint(cx, cy, innerR, a1 - dInner);
    var pInnerStart = polarPoint(cx, cy, innerR, a0 + dInner);
    var pSideStartInner = polarPoint(cx, cy, innerR + crInner, a0);
    var pSideStartOuter = polarPoint(cx, cy, outerR - crOuter, a0);
    return "M " + pOuterStart[0] + " " + pOuterStart[1] +
      " A " + outerR + " " + outerR + " 0 0 1 " + pOuterEnd[0] + " " + pOuterEnd[1] +
      " A " + crOuter + " " + crOuter + " 0 0 1 " + pSideEndOuter[0] + " " + pSideEndOuter[1] +
      " L " + pSideEndInner[0] + " " + pSideEndInner[1] +
      " A " + crInner + " " + crInner + " 0 0 1 " + pInnerEnd[0] + " " + pInnerEnd[1] +
      " A " + innerR + " " + innerR + " 0 0 0 " + pInnerStart[0] + " " + pInnerStart[1] +
      " A " + crInner + " " + crInner + " 0 0 1 " + pSideStartInner[0] + " " + pSideStartInner[1] +
      " L " + pSideStartOuter[0] + " " + pSideStartOuter[1] +
      " A " + crOuter + " " + crOuter + " 0 0 1 " + pOuterStart[0] + " " + pOuterStart[1] +
      " Z";
  }

  var nodeLayer = el("g", { id: "node-layer" });
  svg.appendChild(nodeLayer);
  var nodeGroups = [];
  // Each real (non-guidance) node's REST geometry, keyed by id -- needed
  // after initial draw too, since hover/neighbor growth recomputes and
  // re-sets the wedge's own `d` attribute rather than using a CSS
  // transform (see the .node path doc comment in STATIC_CSS for why).
  var wedgeGeomById = {};
  nodes.forEach(function (n) {
    // ADR-079: guidance nodes never get a chord-graph <g> -- but
    // nodeGroups still gets a placeholder pushed for them (null, not
    // skipped) so it stays index-aligned with `nodes`/`n._idx`: groupFor(id)
    // below looks up nodeGroups[nodesById[id]._idx], and every call site
    // already null-checks its result (e.g. "if (g) { ... }"), so a
    // placeholder is a correct, silent no-op everywhere it's read.
    if (n.is_guidance) { nodeGroups.push(null); return; }
    // Every wedge is exactly minWorldRadius thick -- deliberately UNIFORM,
    // not anchor/degree-scaled (an earlier version added a per-node bonus
    // here, up to +5.4 world units). At real KB node counts that bonus was
    // a large fraction of the base thickness (confirmed on a real 168-node
    // export: thickness ranged 139-168, a ~20% swing) -- since every
    // wedge sits at the exact SAME radius (upstream chord_ring_positions
    // guarantees this), that swing reads as wedges bulging past their
    // neighbors, exactly the overlapping look the wedge redesign existed
    // to eliminate. Degree is still real, visible signal -- see the
    // fill-opacity below, which encodes it without touching geometry.
    var halfThickness = minWorldRadius;
    var nodeRadius = Math.sqrt(
      (n.x - centerX) * (n.x - centerX) + (n.y - centerY) * (n.y - centerY)
    );
    var angle = Math.atan2(n.y - centerY, n.x - centerX);
    // Always exactly the node's own nominal angular slot -- NEVER grown
    // past it to chase a bigger on-screen hit target. An earlier version
    // grew halfSpan up to a 24px-tangential-width floor (mirroring the
    // radial floor above) when a node's own slot was too thin; confirmed
    // as a real, severe bug on a real 168-node export: that floor forced
    // halfSpan past the nominal slot on 142 of 168 wedge boundaries (85%),
    // by MORE than a full slot width in places -- not "a touch of
    // overlap," systemic overlap that defeated the entire point of the
    // wedge redesign (eliminating the old overlapping-circles look). At
    // extreme node counts, a guaranteed minimum tangential hit target and
    // zero overlap are mutually exclusive in finite screen space -- this
    // file picks zero overlap as the hard invariant every time, and lets
    // the hit target degrade gracefully instead (the fullscreen toggle
    // exists specifically to claw some of that back by growing the ring's
    // on-screen size).
    var halfSpan = angleStep / 2 - wedgeGapRadians / 2;
    // "Flower petal" corner rounding (user request), scaled to the
    // wedge's own (rest-state) thickness so it looks proportionate at
    // any node count/ring size -- kept fixed across hover/neighbor growth
    // (refreshWedgeGrowth reuses this same value from wedgeGeomById
    // rather than rescaling it live) so the rounding doesn't visibly
    // change shape mid-transition, only the outer radius does.
    var cornerRadius = halfThickness * (chordConfig.wedgeCornerRadiusFraction ?? 0.6);
    wedgeGeomById[n.id] = {
      nodeRadius: nodeRadius,
      halfThickness: halfThickness,
      angle: angle,
      halfSpan: halfSpan,
      cornerRadius: cornerRadius,
    };
    var g = el("g", {
      class: "node" + (n.is_anchor ? " node-anchor" : ""),
      "data-idx": n._idx,
      "data-kind": n.kind,
      "data-id": n.id,
      // Roving tabindex (standard ARIA pattern for a set of related
      // items): only the currently-selected node is Tab-reachable, kept
      // in sync by updateRovingTabindex() below on every selection
      // change. -1 here is just the safe default until the first
      // applySelection() call sets the real state.
      tabindex: "-1",
      role: "button",
      "aria-label": n["title_" + currentLang],
    });
    var innerR = nodeRadius - halfThickness;
    var outerR = nodeRadius + halfThickness;
    var wedge = el("path", {
      d: arcPath(
        centerX, centerY, innerR, outerR, angle - halfSpan, angle + halfSpan, cornerRadius
      ),
    });
    g.appendChild(wedge);
    // Visited-node marker: an inner ~2/5-thickness band of the wedge
    // itself (a nested smaller arc, not a dot) -- a fill/opacity toggle
    // only, deliberately NOT the fill/stroke/geometry channels hover/
    // neighbor/selected already own on the OUTER wedge (see the .visited
    // CSS below), so it never competes with those. Same angular span as
    // the outer wedge (angle +/- halfSpan).
    //
    // Corner radii are asymmetric (arcPath's two-radius form), NOT the
    // wedge's own cornerRadius scaled down on both edges as a first
    // version tried and a real reported visual mismatch corrected: this
    // band's OUTER edge (innerArcOuterR) is an artificial cut partway
    // through the wedge, not a true wedge boundary -- rounding it made
    // the marker look like a separate rounded pill floating inside the
    // petal instead of lining up flush with the petal's own straight
    // sides. 0 there keeps that edge sharp and flush. Its INNER edge
    // (innerR) IS a real, shared boundary with the wedge itself, so it
    // reuses the wedge's own (unscaled) `cornerRadius` there, nesting
    // against the wedge's own rounded inner corner instead of a
    // mismatched smaller one.
    //
    // Positioned once at draw time since it doesn't move with growth
    // (growth only changes the OUTER wedge's outer radius, see
    // refreshWedgeGrowth).
    var innerArcOuterR = innerR + (outerR - innerR) * 0.4;
    var visitedArc = el("path", {
      class: "visited-inner-arc",
      d: arcPath(
        centerX, centerY, innerR, innerArcOuterR, angle - halfSpan, angle + halfSpan, 0, cornerRadius
      ),
    });
    g.appendChild(visitedArc);
    g.addEventListener("mouseenter", function () { onHover(n, true); });
    g.addEventListener("mousemove", movePopover);
    g.addEventListener("mouseleave", function () { onHover(n, false); });
    g.addEventListener("click", function () { selectNode(n.id); });
    nodeLayer.appendChild(g);
    nodeGroups.push(g);
  });

  // --- Keyboard navigation around the ring (roving tabindex) ---
  //
  // ringOrder: topic node ids in real angular order (NOT `nodes` array
  // order, which reflects layout/insertion order upstream, not position on
  // the ring) -- built from wedgeGeomById's own angle, the same value the
  // draw loop just used, so ArrowLeft/Right always match what's visually
  // adjacent on screen.
  var ringOrder = topicNodes
    .map(function (n) { return n.id; })
    .filter(function (id) { return wedgeGeomById[id]; })
    .sort(function (a, b) { return wedgeGeomById[a].angle - wedgeGeomById[b].angle; });

  // Standard ARIA roving-tabindex bookkeeping: exactly one node (the
  // current selection) is ever Tab-reachable at a time; every other node
  // drops out of the tab order entirely rather than requiring N tab-stops
  // to cross the ring. Called from applySelection on every real selection
  // change, so it's always in sync with `selectedId`.
  function updateRovingTabindex() {
    nodeGroups.forEach(function (ng) {
      if (!ng) { return; }
      ng.setAttribute("tabindex", ng.getAttribute("data-id") === selectedId ? "0" : "-1");
    });
  }

  // ArrowLeft/Right: move around the ring (both focus and selection
  // together -- there's no separate "focused but not yet selected" state
  // in this widget, unlike a typical roving-tabindex menu, since every
  // node is cheap to preview via its detail panel the instant it's
  // reached). ArrowUp/Down deliberately reuse the SAME Next/Previous
  // reading-order buttons Tab/click already drive (not a second,
  // competing way to move through the guide) -- "left/right" means
  // "around the ring," "up/down" means "forward/backward through the
  // guide," a deliberate split so the two never fight over the same keys.
  // Enter/Space activates the focused node the same way a click does --
  // almost always a no-op in practice (focus already mirrors selectedId
  // after every move above), kept anyway so this still behaves like a
  // real `role="button"` for a reader tabbing in fresh.
  nodeLayer.addEventListener("keydown", function (ev) {
    if (ev.key === "ArrowRight" || ev.key === "ArrowLeft") {
      var idx = ringOrder.indexOf(selectedId);
      if (idx === -1) { return; }
      var delta = ev.key === "ArrowRight" ? 1 : -1;
      var nextId = ringOrder[(idx + delta + ringOrder.length) % ringOrder.length];
      selectNode(nextId);
      var nextG = groupFor(nextId);
      if (nextG) { nextG.focus(); }
      ev.preventDefault();
    } else if (ev.key === "ArrowDown") {
      nextBtn.click();
      var afterNextG = groupFor(selectedId);
      if (afterNextG) { afterNextG.focus(); }
      ev.preventDefault();
    } else if (ev.key === "ArrowUp") {
      prevBtn.click();
      var afterPrevG = groupFor(selectedId);
      if (afterPrevG) { afterPrevG.focus(); }
      ev.preventDefault();
    } else if (ev.key === "Enter" || ev.key === " ") {
      selectNode(selectedId);
      ev.preventDefault();
    }
  });

  // Recomputes and re-sets a node's wedge `d` attribute for its CURRENT
  // combined hover/neighbor/rest state -- growth is real geometry (the
  // outer arc's radius), not a CSS transform (see the .node path doc
  // comment in STATIC_CSS for why that approach was tried and reverted).
  // Grows OUTWARD ONLY (inner radius and angular span stay fixed): the
  // wedge "pops out" of the ring rather than also widening sideways into
  // its neighbors' gaps, or inward into the ring's own hollow center.
  // Same precedence as the old CSS had (.hovered's larger growth wins
  // over .neighbor when both apply) -- now enforced by this one function
  // reading class state directly instead of two competing CSS selectors.
  function refreshWedgeGrowth(id) {
    var geom = wedgeGeomById[id];
    var g = groupFor(id);
    if (!geom || !g) { return; }
    // Growth is an ABSOLUTE bonus in world units, not a multiplier on
    // halfThickness itself -- halfThickness (the anchor/degree-scaled
    // reach beyond the ring) is often small relative to nodeRadius (the
    // ring's own large radius), so multiplying IT by 1.25 produced a
    // barely-perceptible change (confirmed empirically: <1% area growth
    // on this session's own fixture, nowhere near the old circles'
    // visibly-lifted hover state). minWorldRadius is the one quantity
    // already calibrated to a real, visible 12px on-screen floor
    // (see its own definition above) -- scaling growth off THAT instead
    // guarantees a real, visible "pop out" regardless of how thin a
    // given node's own halfThickness happens to be. The hover bonus
    // reuses HOVER_GROWTH_FACTOR (defined with the viewBox `pad` fit
    // above) rather than a second, independently-hardcoded 0.6 -- the pad
    // budget and the actual growth applied here MUST stay in sync, or a
    // hovered wedge can clip the viewBox edge (confirmed a real bug this
    // session when they drifted apart).
    var growthBonus = g.classList.contains("hovered")
      ? minWorldRadius * (HOVER_GROWTH_FACTOR - 1)
      : (g.classList.contains("neighbor") ? minWorldRadius * 0.35 : 0);
    var innerR = geom.nodeRadius - geom.halfThickness;
    var outerR = geom.nodeRadius + geom.halfThickness + growthBonus;
    var wedge = g.querySelector("path");
    if (wedge) {
      wedge.setAttribute(
        "d",
        arcPath(
          centerX, centerY, innerR, outerR,
          geom.angle - geom.halfSpan, geom.angle + geom.halfSpan,
          geom.cornerRadius
        )
      );
    }
  }

  function groupFor(id) {
    var n = nodesById[id];
    return n ? nodeGroups[n._idx] : null;
  }

  // Shows a node's title in #graph-caption, below the diagram, at a real
  // legible size -- see the CSS rule's comment for why this replaced
  // in-SVG label text. Falls back to the currently selected node (not
  // blank) so the caption reads as "what am I looking at" rather than
  // flickering empty every time the cursor leaves a node.
  function updateCaption(n) {
    graphCaption.textContent = n ? n["title_" + currentLang] : "";
  }
  // --- Hover popover (title via textContent, never innerHTML) ---
  function onHover(n, entering) {
    var g = groupFor(n.id);
    if (g) { g.classList.toggle("hovered", entering); }
    refreshWedgeGrowth(n.id);
    if (!entering) {
      popover.hidden = true;
      updateCaption(selectedId != null ? nodesById[selectedId] : null);
      return;
    }
    updateCaption(n);
    popover.textContent = "";
    popover.appendChild(dom("div", { class: "popover-title" }, n["title_" + currentLang]));
    popover.appendChild(dom("div", { class: "popover-body" }, n["preview_" + currentLang]));
    popover.hidden = false;
  }
  // Clamp to the viewport instead of always anchoring bottom-right of the
  // cursor: the chord widget sits in the right-hand sidebar, so a node on
  // the right half of the ring puts the cursor near the viewport's right
  // edge already -- an unclamped popover there rendered mostly off-screen.
  // Flip to whichever side of the cursor actually has room, per axis,
  // independently (a popover can need to flip horizontally, vertically,
  // both, or neither depending on where on the ring the cursor is).
  function movePopover(ev) {
    // onHover already set content + unhid the popover before this fires
    // (mouseenter -> onHover, then mousemove -> this), so its real
    // rendered size is already measurable -- no need to reposition or
    // toggle visibility just to read it.
    var rect = popover.getBoundingClientRect();
    var margin = 8;
    var left = ev.clientX + 14;
    if (left + rect.width + margin > window.innerWidth) {
      left = ev.clientX - rect.width - 14;
    }
    left = Math.max(margin, left);
    var top = ev.clientY + 14;
    if (top + rect.height + margin > window.innerHeight) {
      top = ev.clientY - rect.height - 14;
    }
    top = Math.max(margin, top);
    popover.style.left = left + "px";
    popover.style.top = top + "px";
  }

  // --- Hover-preview + click-to-navigate on in-body links (org-roam-ui-
  // style): every internal link the org-link converter produces inside a
  // rendered node body (an <a> whose href is a fragment-style internal
  // reference, not a real URL) gets the exact same hover popover chord-
  // diagram nodes already have -- same onHover/movePopover, same popover
  // element, same nodesById lookup, nothing new to build for that part.
  // Click actually opens the linked node (selectNode) instead of the
  // browser's default same-page fragment-scroll, which -- since no
  // element in this page actually has that id -- had no visible effect at
  // all; a real, reproducible bug (clicking any in-body link silently did
  // nothing), not just a missing nice-to-have. External https links never
  // match the fragment-prefix check, so they're excluded automatically,
  // and an internal reference that doesn't resolve in *this* curated
  // subgraph's nodesById (a real case -- not every link in a body's
  // source note happens to land inside whatever subgraph got exported)
  // is a silent no-op below for both hover and click, not an error.
  // A source note commonly links to more than what a depth-limited curated
  // export actually includes -- that's expected, not a bug in the curation
  // itself (see the "keep extraction opinionated" writing-style note). But
  // rendering those as normal `<a>` elements gave every unresolved link
  // the same blue, underlined, pointer-cursor appearance as a real one,
  // with nothing happening on click -- indistinguishable from a working
  // link until you actually try it. Unwrap unresolved links into plain
  // text (not just a "disabled-looking" style on the `<a>`) so there's no
  // false affordance at all: no color, no cursor, no focus stop.
  function wireBodyLinks(container) {
    var links = container.querySelectorAll("a[href^='#']");
    Array.prototype.forEach.call(links, function (a) {
      var n = nodesById[a.getAttribute("href").slice(1)];
      if (!n) {
        a.replaceWith(document.createTextNode(a.textContent));
        return;
      }
      a.addEventListener("mouseenter", function () { onHover(n, true); });
      a.addEventListener("mousemove", movePopover);
      a.addEventListener("mouseleave", function () { onHover(n, false); });
      a.addEventListener("click", function (ev) {
        ev.preventDefault();
        selectNode(n.id);
      });
    });
  }

  // --- Lightweight, self-contained syntax highlighting for src/example
  // blocks. This page ships zero external dependencies (no CDN script, no
  // bundled third-party highlighter) -- highlighting is a small
  // regex/scan-based tokenizer run over each block's OWN text after body
  // HTML lands in the DOM, not a real language grammar. It recognizes only
  // the token shapes that actually appear in this KB's HCL/Terraform and
  // shell content (comments, strings, numbers, keywords, `${...}`
  // interpolation) -- no speculative generality for languages this KB
  // doesn't use.
  var HL_KEYWORDS = {
    hcl: ["resource", "data", "variable", "output", "module", "provider", "terraform",
          "locals", "for_each", "count", "if", "else", "for", "in", "true", "false", "null"],
    terraform: ["resource", "data", "variable", "output", "module", "provider", "terraform",
                "locals", "for_each", "count", "if", "else", "for", "in", "true", "false", "null"],
    tf: ["resource", "data", "variable", "output", "module", "provider", "terraform",
         "locals", "for_each", "count", "if", "else", "for", "in", "true", "false", "null"],
    shell: ["if", "then", "else", "fi", "for", "in", "do", "done", "while", "case", "esac",
            "function", "return", "export", "local", "echo"],
    bash: ["if", "then", "else", "fi", "for", "in", "do", "done", "while", "case", "esac",
           "function", "return", "export", "local", "echo"],
    sh: ["if", "then", "else", "fi", "for", "in", "do", "done", "while", "case", "esac",
         "function", "return", "export", "local", "echo"]
  };

  function hlEscape(s) {
    // `/[<]/` (a character class), not the more obvious `/</` -- this
    // whole GRAPH_JS text gets a blanket `"</" -> "<\/"` pass
    // (`escape_for_inline_script`) as defense against embedded content
    // prematurely closing the page's own `<script>` tag. That pass is
    // safe inside string literals (`\/` is just an escaped `/`), but a
    // bare `/</g` regex literal has its OWN closing delimiter sitting
    // right after the `<` -- escaping THAT slash strips the regex's
    // closing delimiter and corrupts the whole script (found by actually
    // parsing the exported page's JS with `node --check`, not by
    // inspection -- a real parse failure, not a logic bug).
    return s.replace(/&/g, "&amp;").replace(/[<]/g, "&lt;").replace(/>/g, "&gt;");
  }

  // Tokenizes `src` (plain decoded text, NOT html-escaped) into an HTML
  // string with <span class="tok-*"> around comments/strings/numbers/
  // keywords/HCL `${...}` interpolation; everything else passes through
  // hlEscape()'d and unwrapped. One linear left-to-right scan. A string
  // literal is consumed as a single atomic token (its content is never
  // re-scanned for nested comments/interpolation), which means `${...}`
  // interpolation *inside* a string renders as part of that string's
  // color rather than its own span -- a real limitation of a lightweight
  // scanner, accepted rather than building a real recursive grammar for
  // it.
  function highlightSource(src, keywords) {
    var out = "";
    var i = 0;
    var n = src.length;
    while (i < n) {
      var ch = src[i];
      if (/^#$/.test(ch) || (ch === "/" && src[i + 1] === "/")) {
        var cEnd = src.indexOf("\n", i);
        if (cEnd === -1) { cEnd = n; }
        out += "<span class=\"tok-com\">" + hlEscape(src.slice(i, cEnd)) + "</span>";
        i = cEnd;
        continue;
      }
      if (ch === "/" && src[i + 1] === "*") {
        var bClose = src.indexOf("*/", i + 2);
        var bEnd = bClose === -1 ? n : bClose + 2;
        out += "<span class=\"tok-com\">" + hlEscape(src.slice(i, bEnd)) + "</span>";
        i = bEnd;
        continue;
      }
      if (ch === "\"") {
        var j = i + 1;
        while (j < n && src[j] !== "\"") {
          j += src[j] === "\\" ? 2 : 1;
        }
        j = Math.min(j + 1, n);
        out += "<span class=\"tok-str\">" + hlEscape(src.slice(i, j)) + "</span>";
        i = j;
        continue;
      }
      if (ch === "$" && src[i + 1] === "{") {
        var depth = 1;
        var k = i + 2;
        while (k < n && depth > 0) {
          if (src[k] === "{") { depth++; } else if (src[k] === "}") { depth--; }
          k++;
        }
        out += "<span class=\"tok-interp\">" + hlEscape(src.slice(i, k)) + "</span>";
        i = k;
        continue;
      }
      if (/[0-9]/.test(ch) && (i === 0 || !/[A-Za-z0-9_]/.test(src[i - 1]))) {
        var numMatch = /^[0-9]+(\.[0-9]+)?/.exec(src.slice(i))[0];
        out += "<span class=\"tok-num\">" + numMatch + "</span>";
        i += numMatch.length;
        continue;
      }
      if (/[A-Za-z_]/.test(ch)) {
        var word = /^[A-Za-z_][A-Za-z0-9_]*/.exec(src.slice(i))[0];
        out += keywords.indexOf(word) !== -1
          ? "<span class=\"tok-kw\">" + word + "</span>"
          : hlEscape(word);
        i += word.length;
        continue;
      }
      out += hlEscape(ch);
      i += 1;
    }
    return out;
  }

  // Runs over every `pre code[class^="language-"]` (skipping "mermaid" --
  // already replaced with real inline <svg> or a raw-source fallback, see
  // render_mermaid_block) and every `pre.example` -- the latter gets a
  // narrower treatment: only a leading "$ " shell-prompt marker per line
  // is styled, since example blocks are transcripts (mixed commands and
  // arbitrary output), not one known language.
  function highlightCodeBlocks(container) {
    var blocks = container.querySelectorAll("pre code[class^=\"language-\"]");
    Array.prototype.forEach.call(blocks, function (code) {
      var lang = code.className.slice("language-".length);
      if (lang === "mermaid") { return; }
      code.innerHTML = highlightSource(code.textContent, HL_KEYWORDS[lang] || []);
    });
    var examples = container.querySelectorAll("pre.example");
    Array.prototype.forEach.call(examples, function (pre) {
      var lines = pre.textContent.split("\n");
      pre.innerHTML = lines.map(function (line) {
        if (line.slice(0, 2) === "$ ") {
          return "<span class=\"tok-prompt\">$</span> " + hlEscape(line.slice(2));
        }
        return hlEscape(line);
      }).join("\n");
    });
  }

  // --- Selection / detail panel ---
  function outgoingLinks(id) {
    return edges.filter(function (e) { return e.source === id; })
      .map(function (e) { return { node: nodesById[e.target], rel: e.rel_type }; })
      .filter(function (x) { return x.node; });
  }
  function incomingLinks(id) {
    return edges.filter(function (e) { return e.target === id; })
      .map(function (e) { return { node: nodesById[e.source], rel: e.rel_type }; })
      .filter(function (x) { return x.node; });
  }

  function renderLinkList(container, title, links) {
    if (links.length === 0) { return; }
    container.appendChild(dom("h3", {}, title));
    var ul = dom("ul", { class: "link-list" });
    links.forEach(function (l) {
      var li = dom("li");
      var btn = dom("button", { type: "button", class: "link-jump" });
      btn.appendChild(document.createTextNode(l.node["title_" + currentLang] + " "));
      btn.appendChild(dom("span", { class: "external-link" }, "(" + (l.rel || "related_to") + ")"));
      btn.addEventListener("click", function () { selectNode(l.node.id); });
      li.appendChild(btn);
      ul.appendChild(li);
    });
    container.appendChild(ul);
  }

  // --- "On this page" outline: scanned from the ACTUAL rendered heading
  // elements inside .detail-body (not a second, possibly-diverging parse)
  // -- single source of truth is whatever really ended up in the DOM. ---
  function renderOutline(bodyEl) {
    outlineList.textContent = "";
    var headings = bodyEl.querySelectorAll("h1, h2, h3, h4, h5, h6");
    if (headings.length === 0) { outlinePanel.hidden = true; return; }
    outlinePanel.hidden = false;
    headings.forEach(function (h, i) {
      var id = "outline-h-" + i;
      h.id = id;
      var li = dom("li");
      var btn = dom("button", { type: "button" }, h.textContent);
      btn.style.paddingLeft = (Math.max(0, (parseInt(h.tagName.substring(1), 10) - 1)) * 0.75) + "rem";
      btn.addEventListener("click", function () {
        h.scrollIntoView({ behavior: "smooth", block: "start" });
      });
      li.appendChild(btn);
      outlineList.appendChild(li);
    });
  }
  outlineToggle.addEventListener("click", function () {
    outlinePanel.classList.toggle("collapsed");
  });

  // --- Visited-node history panel: renders visitStack/visitPos, kept in
  // sync by selectNode() (new navigation) and the popstate listener
  // (Back/Forward replay) above. Entries are real <button>s wired to
  // selectNode -- clicking any past (or, after a Back, future) entry
  // jumps straight there, same real-navigation path as everything else on
  // the page. The current node is not a button (nothing to click to get
  // somewhere already on screen), just an accent-bordered row. ---
  function renderHistoryPanel() {
    historyList.textContent = "";
    if (visitDropped > 0) {
      historyList.appendChild(dom(
        "li", { class: "history-truncated" },
        "⋯ " + visitDropped + " earlier"
      ));
    }
    visitStack.forEach(function (id, i) {
      var n = nodesById[id];
      if (!n) { return; }
      var li = dom("li");
      if (i === visitPos) {
        var row = dom("span", { class: "history-current" }, n["title_" + currentLang]);
        li.appendChild(row);
      } else {
        var btn = dom("button", { type: "button" }, n["title_" + currentLang]);
        btn.addEventListener("click", function () { selectNode(id); });
        li.appendChild(btn);
        if (i === visitPos - 1) {
          li.appendChild(dom("span", { class: "history-marker" }, "← Back"));
        } else if (i === visitPos + 1) {
          li.appendChild(dom("span", { class: "history-marker" }, "Forward →"));
        }
      }
      historyList.appendChild(li);
    });
    historyBackBtn.disabled = visitPos <= 0;
    historyForwardBtn.disabled = visitPos >= visitStack.length - 1;
  }
  // Back/Forward buttons replay through the REAL browser history (not a
  // second, hand-rolled navigation path) -- history.back()/forward()
  // trigger the SAME popstate listener above that already reconciles
  // visitStack/visitPos, so there is exactly one place that logic lives.
  historyBackBtn.addEventListener("click", function () { history.back(); });
  historyForwardBtn.addEventListener("click", function () { history.forward(); });

  function renderDetail(n) {
    detailContent.classList.add("fading");
    window.setTimeout(function () {
      detailContent.textContent = "";
      detailContent.appendChild(dom("span", { class: "kind-badge" }, n.kind));
      // "Part ::" is a label from the node's own authored Reading Order
      // section (parse_reading_order_part, Rust side) -- structural
      // context ("where am I in the guide"), not a navigable link, so it
      // renders as plain muted text, never wrapped in an <a>. Per-language
      // like title/body (falls back to English server-side when no
      // translation exists -- see build_export_node), absent entirely for
      // nodes with no Reading Order section at all.
      var partLabel = n["reading_order_part_" + currentLang];
      if (partLabel) {
        detailContent.appendChild(dom("div", { class: "node-part-breadcrumb" }, partLabel));
      }
      detailContent.appendChild(dom("h2", { class: "detail-title" }, n["title_" + currentLang]));
      if (n.is_anchor) {
        detailContent.appendChild(dom(
          "p", { class: "anchor-note" },
          "Starting point of this exported subgraph."
        ));
      }
      // ADR-079: a reader can also open a guidance/colophon node
      // directly (colophon button, or an ordinary in-body link to one) --
      // this note orients them the same way anchor-note does for the
      // anchor, since nothing else on this screen says "you left the
      // guide's own topic content."
      if (n.is_guidance) {
        detailContent.appendChild(dom(
          "p", { class: "guidance-note" },
          "Guidance note — a standard this guide was written against, not part of its topic content."
        ));
      }
      // ADR-078: the language
      // toggle is a real, working GLOBAL preference -- it must keep
      // applying even on a node with no Spanish translation, so it's
      // never disabled per-node. But title_es/body_es mirroring title_en/
      // body_en exactly (the deliberate fallback for "no translation
      // exists") previously gave no visible signal at all when a reader
      // toggled to Spanish on one of those nodes: the button's own label
      // changed, the content didn't, and a reader clicking it repeatedly
      // reasonably concluded the switch was broken. Surface the fallback
      // per field (title/body can be translated independently) rather
      // than only per-node, so a partial translation doesn't silently
      // read as complete either.
      if (currentLang === "es") {
        var titleFallback = n.title_es === n.title_en;
        var bodyFallback = n.body_es === n.body_en;
        var fallbackMsg = null;
        if (titleFallback && bodyFallback) {
          fallbackMsg = "This note isn't translated yet — showing English.";
        } else if (titleFallback) {
          fallbackMsg = "This note's title isn't translated yet — showing the English title.";
        } else if (bodyFallback) {
          fallbackMsg = "This note's text isn't translated yet — showing the English text.";
        }
        if (fallbackMsg) {
          detailContent.appendChild(dom(
            "p", { class: "translation-fallback-note" }, fallbackMsg
          ));
        }
      }
      var body = dom("div", { class: "detail-body" });
      // n.body_en / n.body_es are pre-escaped HTML produced server-side by
      // mae-export's org renderer (crate::html_escape on every bit of real
      // node content, plus pre-rendered mermaid <svg>) -- this is the ONE
      // deliberate innerHTML assignment in this file; every other piece of
      // text above/below goes through textContent/dom() instead.
      body.innerHTML = n["body_" + currentLang];
      wireBodyLinks(body);
      highlightCodeBlocks(body);
      detailContent.appendChild(body);
      renderLinkList(detailContent, "Links to", outgoingLinks(n.id));
      renderLinkList(detailContent, "Linked from", incomingLinks(n.id));
      renderOutline(body);
      detailContent.classList.remove("fading");
    }, 120);
  }

  // Applies a selection to the DOM only -- no history side effect. Used by
  // both real navigation (selectNode, below) and the popstate handler
  // (browser back/forward), which must NOT push a new entry for a
  // navigation the browser is already replaying.
  function applySelection(id) {
    var n = nodesById[id];
    if (!n) { return; }
    // Every real navigation starts at the top of the new node's content --
    // #main-content is the actual scrolling container (overflow-y: auto),
    // not the window, so a plain anchor-jump/scrollIntoView wouldn't do
    // this on its own. Confirmed as a real reported bug: following an
    // in-body link while scrolled partway down the current node left the
    // reader at that same scroll offset on the newly-loaded node's
    // content, which reads as "did this even navigate?" if the new
    // content happens to be shorter than the old scroll position.
    if (mainContent) { mainContent.scrollTop = 0; }
    // A click-to-navigate (chord node or in-body link) doesn't reliably
    // fire the hovered element's mouseleave -- clicking a body link
    // replaces #main-content's DOM (including the very <a> under the
    // cursor) as part of this call, and the popover was observed staying
    // on screen indefinitely afterward. Navigating away always ends
    // whatever hover context produced the popover, regardless of why the
    // browser didn't fire mouseleave for it.
    popover.hidden = true;
    if (selectedId != null) {
      var prevG = groupFor(selectedId);
      if (prevG) { prevG.classList.remove("selected"); }
    }
    selectedId = id;
    visitedIds[id] = true;
    // Persist the current node so reopening this same exported file later
    // resumes here instead of always restarting at the anchor (user
    // request) -- same try/catch-wrapped, per-file-path localStorage
    // pattern the theme preference below already uses; see its own
    // comment for why some privacy modes throw here. Updated on every
    // real selection (including Back/Forward replays via popstate calling
    // applySelection directly), not just forward navigation, since
    // "resume where you left off" should reflect wherever the reader
    // actually ended up.
    try { localStorage.setItem("mae-guide-last-node", id); } catch (e) { /* ignore */ }
    updateCaption(n);
    var g = groupFor(id);
    if (g) { g.classList.add("selected"); }
    updateRovingTabindex();
    // Visited marker: every node ever selected gets `.visited`, EXCEPT
    // the currently-selected one -- selected already owns the
    // fill/stroke/geometry channels, so showing the visited dot on top of
    // it would just be visual noise for information the selected styling
    // already conveys on its own.
    topicNodes.forEach(function (tn) {
      var tg = groupFor(tn.id);
      if (!tg) { return; }
      tg.classList.toggle("visited", !!visitedIds[tn.id] && tn.id !== id);
    });
    // Directly-linked neighbor nodes get their own highlight (bigger hit
    // target via the same transform: scale mechanism .hovered already
    // uses, plus a distinct ring color) -- not just their connecting
    // edges. Confirmed a real, reported gap: in a dense ring the OTHER
    // endpoint of a highlighted edge looked identical to every unrelated
    // node, hard to both spot and precisely click. Cleared from every
    // node first (simpler and just as correct as tracking the previous
    // neighbor set) then reapplied for the new selection.
    nodeGroups.forEach(function (ng) { if (ng) { ng.classList.remove("neighbor"); } });
    edgePaths.forEach(function (p) {
      var src = p.getAttribute("data-source"), tgt = p.getAttribute("data-target");
      var incident = src === id || tgt === id;
      p.classList.toggle("incident", incident);
      if (incident) {
        var neighborGroup = groupFor(src === id ? tgt : src);
        if (neighborGroup) { neighborGroup.classList.add("neighbor"); }
      }
    });
    // Refresh EVERY topic node's wedge growth, not just the ones that
    // changed -- simpler and just as correct as tracking a precise delta
    // (a node that stopped being a neighbor needs to shrink back too, and
    // this is cheap at the node counts this widget targets).
    topicNodes.forEach(function (tn) { refreshWedgeGrowth(tn.id); });
    renderDetail(n);
  }
  // Real navigation (chord click, body-link click, Home/Previous/Next):
  // pushes a history entry so the browser's own Back/Forward buttons work
  // -- the one navigation UX every reader already knows, and the actual
  // gap Home/Previous/Next (a linear reading-order walk) doesn't cover on
  // its own: following links freely through the graph has no "undo" of
  // its own otherwise. A no-op re-selection of the already-open node
  // (e.g. clicking a link back to the current page) doesn't push a
  // duplicate entry.
  function selectNode(id) {
    if (id === selectedId) { return; }
    if (!nodesById[id]) { return; }
    applySelection(id);
    // Record this as new forward navigation in the visited-history stack,
    // unconditionally -- regardless of whether the pushState call just
    // below succeeds or throws (see its own comment): the navigation
    // itself really happened (applySelection already ran), so the shadow
    // history the panel renders from should reflect that either way.
    // If we're not already at the tail (the reader went Back and is now
    // taking a NEW path), drop everything after the current position first
    // -- the same forward-history invalidation a real browser does the
    // moment you navigate somewhere new after Back.
    if (visitPos < visitStack.length - 1) {
      visitStack = visitStack.slice(0, visitPos + 1);
    }
    visitStack.push(id);
    visitPos = visitStack.length - 1;
    while (visitStack.length > HISTORY_DEPTH_CAP) {
      visitStack.shift();
      visitPos -= 1;
      visitDropped += 1;
    }
    renderHistoryPanel();
    // Single-quoted deliberately, not double: this whole script is a Rust
    // raw string delimited by double-quote-hash, and that exact two-char
    // sequence anywhere in the JS source closes it early (a real compile
    // break hit while writing this).
    //
    // Regression: pushState was called unconditionally, with nothing
    // catching a throw. Firefox rate-limits History API calls under
    // file:// -- clicking through even a modest number of nodes (Next
    // repeatedly, or a few body links) throws a real, reproducible
    // SecurityError ("the operation is insecure") once the limit is hit.
    // An uncaught throw here aborts selectNode() -- and, critically,
    // whatever the CALLER does *after* calling it: nextBtn/prevBtn's
    // click handlers call updateWalkButtons() right after selectNode(),
    // so a thrown pushState left Previous/Next's disabled state stale
    // and, on some navigations, effectively stuck. Content itself still
    // updates correctly (applySelection already ran above) -- only the
    // history entry is lost when this throws, which degrades gracefully
    // to "Back/Forward won't undo this one step" instead of breaking
    // navigation entirely.
    try {
      history.pushState({ nodeId: id }, "", '#' + id);
    } catch (e) { /* ignore -- see comment above */ }
  }
  window.addEventListener("popstate", function (ev) {
    var id = (ev.state && ev.state.nodeId) || anchorId;
    applySelection(id);
    // Keep Previous/Next's position (and disabled state) consistent with
    // whatever Back/Forward just landed on -- every node is present in
    // readingOrder, so this always finds a real index. Without this, a
    // Next click after a Back would continue from wherever walkIndex was
    // left by the last Previous/Next click instead of from the node
    // actually on screen.
    var idx = readingOrder.indexOf(id);
    if (idx !== -1) { walkIndex = idx; }
    updateWalkButtons();
    // Replaying history (native Back/Forward), not making new history --
    // move visitPos to match, never push/truncate. Search outward from the
    // current position first (nearest match is almost always right, and
    // handles a ring or repeat-visited node appearing more than once in
    // visitStack) before falling back to any occurrence.
    var foundAt = -1;
    for (var d = 0; d < visitStack.length && foundAt === -1; d++) {
      if (visitStack[visitPos - d] === id) { foundAt = visitPos - d; }
      else if (visitStack[visitPos + d] === id) { foundAt = visitPos + d; }
    }
    if (foundAt !== -1) {
      visitPos = foundAt;
    } else {
      // Not found at all -- state lost (e.g. a reload) or evicted past the
      // depth cap earlier. Degrade visibly, not silently: reseed from just
      // this one node rather than showing a stale/wrong stack, the same
      // "never break, degrade visibly" posture the pushState try/catch
      // above already takes.
      visitStack = [id];
      visitPos = 0;
      visitDropped = 0;
    }
    renderHistoryPanel();
  });
  // Regression found by this project's Layer 2 browser suite (kb/adrs/
  //0001): Home previously only called selectNode(anchorId), never
  // resetting walkIndex. readingOrder[0] is always the anchor, so after
  // walking forward to some position N via Next, clicking Home visually
  // returns to the anchor -- but a subsequent Next click resumed from the
  // stale walkIndex (N + 1), not from position 1 (the real "next after
  // home"), landing on an unexpected node with no visible sign anything
  // was wrong. Home is conceptually "jump to position 0," so it must
  // reset walkIndex the same way the popstate handler above resyncs it
  // for Back/Forward.
  homeBtn.addEventListener("click", function () {
    selectNode(anchorId);
    walkIndex = anchorWalkIndex();
    updateWalkButtons();
  });

  // --- Reading order: an explicit, authored Previous/Next chain when the
  // source KB has one (a project-local org convention -- see
  // parse_reading_order's Rust doc comment; mae_kb has no first-class
  // concept of this), falling back to BFS-distance-from-anchor + degree +
  // alphabetical tiebreak for any node that isn't part of one. Chain-linked
  // nodes come first, in chain order; everything else is appended after. ---
  function computeReadingOrder() {
    // ADR-079: guidance/colophon nodes never enter the Previous/Next
    // walk -- topicNodes, not nodes, both seeds `dist` (so a guidance node
    // is never a possible destination) and produces the final order.
    var topicIds = {};
    topicNodes.forEach(function (n) { topicIds[n.id] = true; });
    function validPrev(n) { return n.reading_order_prev && topicIds[n.reading_order_prev] ? n.reading_order_prev : null; }
    function validNext(n) { return n.reading_order_next && topicIds[n.reading_order_next] ? n.reading_order_next : null; }

    var visited = {};
    var order = [];
    topicNodes.forEach(function (n) {
      if (visited[n.id] || (!validPrev(n) && !validNext(n))) { return; }
      // Walk backward to this chain segment's start (guarded: real,
      // user-authored data, not a guaranteed-acyclic machine format).
      var startId = n.id, guard = 0;
      while (true) {
        var p = validPrev(nodesById[startId]);
        if (!p || visited[p] || ++guard > topicNodes.length) { break; }
        startId = p;
      }
      // Then forward from the start, collecting the whole segment once.
      var cur = startId, guard2 = 0;
      while (cur && !visited[cur] && guard2++ <= topicNodes.length) {
        visited[cur] = true;
        order.push(cur);
        cur = validNext(nodesById[cur]);
      }
    });

    var adjacency = {};
    topicNodes.forEach(function (n) { adjacency[n.id] = []; });
    edges.forEach(function (e) {
      if (adjacency[e.source]) { adjacency[e.source].push(e.target); }
      if (adjacency[e.target]) { adjacency[e.target].push(e.source); }
    });
    var dist = {};
    topicNodes.forEach(function (n) { dist[n.id] = Infinity; });
    if (dist[anchorId] !== undefined) {
      dist[anchorId] = 0;
      var queue = [anchorId];
      while (queue.length) {
        var cur2 = queue.shift();
        (adjacency[cur2] || []).forEach(function (next) {
          if (dist[next] === Infinity) { dist[next] = dist[cur2] + 1; queue.push(next); }
        });
      }
    }
    var rest = topicNodes.filter(function (n) { return !visited[n.id]; }).sort(function (a, b) {
      if (dist[a.id] !== dist[b.id]) { return dist[a.id] - dist[b.id]; }
      var degA = degreeOf(a.id), degB = degreeOf(b.id);
      if (degA !== degB) { return degB - degA; }
      return a.id < b.id ? -1 : (a.id > b.id ? 1 : 0);
    });
    return {
      ids: order.concat(rest.map(function (n) { return n.id; })),
      // Which ids were actually chain-walked (`visited`, reused directly --
      // a real KB can have MORE THAN ONE independent authored chain, e.g.
      // gitlab-migration's own main project-wide sequence PLUS a separate
      // local one inside gitlab-platform/gitlab-host's own ADRs; both get
      // walked and concatenated into `order`, so a single "chain ends at
      // index N" boundary is wrong -- confirmed on that exact 167-node
      // export: it stopped Next one click too late, at the node just past
      // the MAIN chain's real end, because that node happened to belong to
      // the second chain). See updateWalkButtons below for how this is
      // used: per-node, not as a single prefix-length boundary.
      isChainNode: visited,
      topicIds: topicIds,
    };
  }
  // Previous/Next share one position in `readingOrder`, clamped (not
  // modulo-wrapped) at both ends -- Next stops at the last node instead of
  // silently wrapping back to the start, so the two controls behave like
  // ordinary pagination, each disabled exactly when it has nowhere to go.
  //
  // walkIndex starts at the ANCHOR's own position in readingOrder, not a
  // hardcoded 0: when the KB has an explicit authored chain, readingOrder
  // follows THAT order first (see computeReadingOrder above), and the
  // anchor -- whichever node the export was actually rooted at -- can
  // legitimately sit anywhere within it, not just at the start. The anchor
  // is already auto-selected on page load (see selectNode(anchorId) below),
  // so walkIndex must point at wherever it really landed for position 0 to
  // mean "what's already on screen" -- exactly the invariant that made
  // starting at a hardcoded 0 correct back when readingOrder[0] was always
  // the anchor by construction (pure BFS distance from itself is always
  // zero); that invariant no longer holds unconditionally, so this is
  // computed instead of assumed.
  var readingOrderResult = computeReadingOrder();
  var readingOrder = readingOrderResult.ids;
  var isChainNode = readingOrderResult.isChainNode;
  var readingOrderTopicIds = readingOrderResult.topicIds;
  function anchorWalkIndex() {
    var i = readingOrder.indexOf(anchorId);
    return i === -1 ? 0 : i;
  }
  var walkIndex = anchorWalkIndex();
  // Next stops at the authored chain's real end (a genuine "Next :: none"
  // boundary, per the KB's own Reading Order data) rather than spilling
  // into unrelated BFS-fallback content -- confirmed as a real, jarring UX
  // gap on a real 167-node export (walking off "README" straight into
  // unrelated roadmap/ADR material with no signal anything had changed).
  // Checked per-node, not via a single fixed boundary: a KB can have more
  // than one independent authored chain (that same export has a second,
  // separate one inside gitlab-platform/gitlab-host's own ADRs) -- Next
  // follows whichever chain the CURRENT node is actually on to ITS OWN
  // real end, only stopping there, rather than stopping at wherever the
  // FIRST-discovered chain happened to end. A node never on any chain
  // (isChainNode false) still gets ordinary end-of-list pagination,
  // unaffected -- BFS-fallback nodes remain reachable directly (chord
  // ring, search, colophon), just not via Next once the current chain
  // (or, for chain-less nodes, the whole list) is done.
  function atChainEnd() {
    var n = nodesById[readingOrder[walkIndex]];
    if (!n || !isChainNode[n.id]) { return false; }
    return !(n.reading_order_next && readingOrderTopicIds[n.reading_order_next]);
  }
  function updateWalkButtons() {
    prevBtn.disabled = walkIndex <= 0;
    var done = walkIndex >= readingOrder.length - 1 || atChainEnd();
    nextBtn.textContent = done ? "✓ Done" : "Next →";
    nextBtn.disabled = done;
  }
  nextBtn.addEventListener("click", function () {
    walkIndex = Math.min(walkIndex + 1, readingOrder.length - 1);
    selectNode(readingOrder[walkIndex]);
    updateWalkButtons();
  });
  prevBtn.addEventListener("click", function () {
    if (walkIndex <= 0) { return; }
    walkIndex -= 1;
    selectNode(readingOrder[walkIndex]);
    updateWalkButtons();
  });
  updateWalkButtons();

  // --- Header search: hand-rolled subsequence fuzzy match (no external
  // lib -- this page ships zero dependencies) against each topic node's
  // CURRENT-language title. A match requires every query character to
  // appear in target order; score rewards consecutive-character runs and
  // word-start matches so "gitlab ci" beats a scattered same-length match.
  // A single, distinct effect (jump-to via a dropdown) -- deliberately not
  // entangled with the tag filter's chord-ring dimming below. ---
  function fuzzyScore(query, target) {
    if (!query) { return null; }
    var q = query.toLowerCase(), t = target.toLowerCase();
    var qi = 0, score = 0, consecutive = 0;
    for (var ti = 0; ti < t.length && qi < q.length; ti++) {
      if (t[ti] === q[qi]) {
        consecutive++;
        score += 1 + consecutive;
        if (ti === 0 || /[\s\-_/]/.test(t[ti - 1])) { score += 3; }
        qi++;
      } else {
        consecutive = 0;
      }
    }
    return qi === q.length ? score : null;
  }
  function renderSearchResults(query) {
    searchResults.textContent = "";
    if (!query) { searchResults.hidden = true; return; }
    var scored = topicNodes.map(function (n) {
      return { node: n, score: fuzzyScore(query, n["title_" + currentLang]) };
    }).filter(function (x) { return x.score !== null; })
      .sort(function (a, b) { return b.score - a.score; })
      .slice(0, 8);
    if (scored.length === 0) { searchResults.hidden = true; return; }
    scored.forEach(function (x) {
      var btn = dom("button", { type: "button" }, x.node["title_" + currentLang]);
      btn.addEventListener("click", function () {
        selectNode(x.node.id);
        nodeSearch.value = "";
        searchResults.hidden = true;
      });
      searchResults.appendChild(btn);
    });
    searchResults.hidden = false;
  }
  nodeSearch.addEventListener("input", function () {
    renderSearchResults(nodeSearch.value.trim());
  });
  nodeSearch.addEventListener("keydown", function (ev) {
    if (ev.key === "Escape") { searchResults.hidden = true; nodeSearch.blur(); }
  });
  // Delayed hide on blur (not immediate) -- a click on a result row blurs
  // the input just before its own click handler would otherwise fire;
  // hiding synchronously on blur would remove the button from the DOM
  // before that click registers.
  nodeSearch.addEventListener("blur", function () {
    window.setTimeout(function () { searchResults.hidden = true; }, chordConfig.searchDebounceMs ?? 150);
  });

  // --- Header tag filter: dims (never removes) non-matching nodes/edges
  // in the chord ring -- the graph itself becomes the filtered view, no
  // separate list. OR semantics across active tags (the standard choice
  // for one flat facet -- AND would too easily produce an empty result on
  // sparse tag combinations). Guidance nodes are already excluded from
  // topicNodes (ADR-079), so they're never part of this either. ---
  var allTags = [];
  (function () {
    var seen = {};
    topicNodes.forEach(function (n) {
      (n.tags || []).forEach(function (t) {
        if (!seen[t]) { seen[t] = true; allTags.push(t); }
      });
    });
    allTags.sort();
  })();
  if (allTags.length === 0) { tagFilterGroup.hidden = true; }
  var activeTagFilters = {};
  function nodeMatchesTagFilter(n) {
    var active = Object.keys(activeTagFilters);
    if (active.length === 0) { return true; }
    return (n.tags || []).some(function (t) { return activeTagFilters[t]; });
  }
  function applyTagFilter() {
    nodes.forEach(function (n, i) {
      var g = nodeGroups[i];
      if (!g) { return; }
      g.classList.toggle("filtered-out", !nodeMatchesTagFilter(n));
    });
    edgePaths.forEach(function (p) {
      var s = nodesById[p.getAttribute("data-source")];
      var t = nodesById[p.getAttribute("data-target")];
      var bothMatch = s && t && nodeMatchesTagFilter(s) && nodeMatchesTagFilter(t);
      p.classList.toggle("filtered-out", !bothMatch);
    });
  }
  function renderTagPicker() {
    tagPicker.textContent = "";
    allTags.forEach(function (t) {
      var btn = dom("button", { type: "button" }, t);
      if (activeTagFilters[t]) { btn.classList.add("active"); }
      btn.addEventListener("click", function () { toggleTagFilter(t); });
      tagPicker.appendChild(btn);
    });
  }
  function renderActiveTagChips() {
    activeTagChips.textContent = "";
    Object.keys(activeTagFilters).forEach(function (t) {
      var btn = dom("button", { type: "button" }, t + " ×");
      btn.addEventListener("click", function () { toggleTagFilter(t); });
      activeTagChips.appendChild(btn);
    });
  }
  function toggleTagFilter(t) {
    if (activeTagFilters[t]) { delete activeTagFilters[t]; } else { activeTagFilters[t] = true; }
    renderTagPicker();
    renderActiveTagChips();
    applyTagFilter();
  }
  tagPickerToggle.addEventListener("click", function () {
    tagPicker.hidden = !tagPicker.hidden;
  });
  document.addEventListener("click", function (ev) {
    if (!tagFilterGroup.contains(ev.target)) { tagPicker.hidden = true; }
  });
  renderTagPicker();
  renderActiveTagChips();
  applyTagFilter();

  // --- Chord diagram fullscreen: an in-page expand, not the native
  // browser Fullscreen API (requestFullscreen()) -- that API needs a
  // user-activation gesture with quirky cross-engine/file:// behavior and
  // hands over the WHOLE SCREEN (including the OS chrome disappearing),
  // which is more than this needs. A `position: fixed` overlay gives the
  // same "big, easy-to-read ring" result while staying simple and
  // reliably testable. #graph-svg's own preserveAspectRatio="xMidYMid
  // meet" already scales the existing viewBox to fill whatever size its
  // container becomes -- no viewBox/geometry recompute needed here, the
  // ring just renders bigger (a real usability win: larger hit targets)
  // once its container grows.
  //
  // Clicking a node while fullscreen still just calls selectNode() (the
  // per-node click listener is unchanged) and does NOT auto-exit --
  // exploring several nodes at the enlarged size is the whole point;
  // making every click bounce back to the small view would defeat it. An
  // explicit toggle (this same button, now showing X) or Escape is the
  // only way out.
  //
  // Enter/exit both use a CSS @keyframes animation (STATIC_CSS,
  // graph-fullscreen-in/out) rather than a transition: `position: fixed`
  // itself isn't an animatable property, and the pane's opacity/transform
  // don't otherwise change between its normal and fullscreen layouts, so
  // there's no "before" state for a transition to interpolate from --
  // this file's small-motion convention (200ms ease) needs a real
  // self-contained animation here instead, not just a transition.
  var isGraphFullscreen = false;
  function setGraphFullscreen(next) {
    if (next === isGraphFullscreen) { return; }
    isGraphFullscreen = next;
    graphPane.classList.remove("fullscreen-anim-in", "fullscreen-anim-out");
    if (next) {
      graphPane.classList.add("fullscreen", "fullscreen-anim-in");
    } else {
      // Stays positioned fullscreen (`.fullscreen` not removed yet) while
      // the shrink-out animation plays, then drops out of fixed
      // positioning once it's actually finished -- removing `.fullscreen`
      // immediately would snap it back to the sidebar's small layout
      // before the animation had anything to animate.
      graphPane.classList.add("fullscreen-anim-out");
    }
    fullscreenToggle.textContent = next ? "✕" : "⛶";
    var label = next ? "Exit fullscreen" : "Expand diagram";
    fullscreenToggle.title = label;
    fullscreenToggle.setAttribute("aria-label", label);
  }
  graphPane.addEventListener("animationend", function (ev) {
    if (ev.animationName === "graph-fullscreen-out") {
      graphPane.classList.remove("fullscreen", "fullscreen-anim-out");
    }
  });
  fullscreenToggle.addEventListener("click", function () {
    setGraphFullscreen(!isGraphFullscreen);
  });
  // The Escape handler for THIS overlay is merged with the sidebar
  // drawer's below into one listener (search "ev.key === \"Escape\"") --
  // two separate `keydown` listeners each reading isGraphFullscreen raced
  // against each other: this one flips it to false, then the sidebar's
  // listener (registered second, so it runs second on the SAME keydown
  // event) reads the ALREADY-flipped value and wrongly concludes nothing
  // was fullscreen, closing the drawer too on the very same press. A
  // single listener that checks fullscreen first and returns early is the
  // only way to peel back one overlay per press.

  // --- #sidebar-toggle: one shared boolean drives BOTH the desktop
  // collapse (instant, #main-content reclaims the width) and the mobile
  // drawer (an off-canvas overlay, same fixed-position-overlay pattern as
  // #graph-pane's own fullscreen above) -- see the STATIC_CSS comment
  // above #sidebar for why a single `data-sidebar` attribute on <html>,
  // not a class, is what makes one control correct at both breakpoints.
  var SIDEBAR_MOBILE_QUERY = "(max-width: 767px)";
  function sidebarIsOpen() {
    var explicit = document.documentElement.getAttribute("data-sidebar");
    if (explicit === "open") { return true; }
    if (explicit === "closed") { return false; }
    // Nothing explicit yet: the plain per-breakpoint CSS default applies
    // (open on desktop, closed on mobile) -- matches sidebarIsOpen()'s own
    // read exactly, so the button's label/aria stay in sync even before
    // any click or stored preference exists.
    return !(window.matchMedia && window.matchMedia(SIDEBAR_MOBILE_QUERY).matches);
  }
  function updateSidebarToggleLabel(open) {
    var label = open ? "Hide sidebar" : "Show sidebar";
    sidebarToggle.textContent = "☰ " + label;
    sidebarToggle.setAttribute("aria-expanded", open ? "true" : "false");
  }
  function setSidebarOpen(open) {
    // Only the mobile drawer, closing from an already-EXPLICITLY-open
    // state, needs the two-phase animate-then-flip dance -- e.g. on
    // initial load with a stored "closed" preference, data-sidebar was
    // never explicitly "open" (nothing was visibly showing), so there's
    // nothing to animate away from; flip straight to closed.
    var wasExplicitlyOpen = document.documentElement.getAttribute("data-sidebar") === "open";
    if (open) {
      document.documentElement.removeAttribute("data-sidebar-anim");
      document.documentElement.setAttribute("data-sidebar", "open");
    } else if (wasExplicitlyOpen && window.matchMedia && window.matchMedia(SIDEBAR_MOBILE_QUERY).matches) {
      // Mobile close: keep data-sidebar="open" (so the fixed/inset:0
      // positioning stays) while the slide-out plays, then the
      // animationend listener below flips it to "closed" -- same
      // two-phase approach as setGraphFullscreen's fullscreen-anim-out.
      document.documentElement.setAttribute("data-sidebar-anim", "out");
    } else {
      // Desktop close has no animation (STATIC_CSS's min-width:768px
      // rule is a plain instant display:none) -- flip immediately.
      document.documentElement.setAttribute("data-sidebar", "closed");
    }
    updateSidebarToggleLabel(open);
    try {
      localStorage.setItem("mae-guide-sidebar-collapsed", open ? "false" : "true");
    } catch (e) { /* ignore */ }
  }
  sidebarEl.addEventListener("animationend", function (ev) {
    if (ev.animationName === "sidebar-drawer-out") {
      document.documentElement.setAttribute("data-sidebar", "closed");
      document.documentElement.removeAttribute("data-sidebar-anim");
    }
  });
  sidebarToggle.addEventListener("click", function () {
    setSidebarOpen(!sidebarIsOpen());
  });
  sidebarBackdrop.addEventListener("click", function () {
    setSidebarOpen(false);
  });
  // One merged Escape handler for BOTH overlays (see the comment above
  // fullscreenToggle's click listener for why splitting this across two
  // `keydown` listeners is a real, caught bug, not just a style choice):
  // fullscreen is checked FIRST and returns immediately, so a single
  // Escape press with both overlays open closes only the topmost one
  // (fullscreen); a second press is needed to close the drawer.
  document.addEventListener("keydown", function (ev) {
    if (ev.key !== "Escape") { return; }
    if (isGraphFullscreen) {
      setGraphFullscreen(false);
      return;
    }
    if (sidebarIsOpen() && window.matchMedia && window.matchMedia(SIDEBAR_MOBILE_QUERY).matches) {
      setSidebarOpen(false);
    }
  });
  // Sync the button's label/aria to whatever's in effect on first paint --
  // a stored preference (read alongside the theme preference below) may
  // still call setSidebarOpen() again after this, which is fine
  // (idempotent for the label/aria, and re-applying "open"/"closed" is a
  // no-op if it's already that value).
  updateSidebarToggleLabel(sidebarIsOpen());

  // --- Colophon (ADR-079): each button opens its guidance node via
  // the SAME selectNode() real navigation (chord nodes, in-body links, and
  // Home/Previous/Next all already funnel through it) -- language toggle,
  // ADR-0003's translation-fallback notice, mermaid, Back/Forward history,
  // all just work with no separate code path to keep in sync. ---
  var colophonLinks = document.querySelectorAll(".colophon-link");
  Array.prototype.forEach.call(colophonLinks, function (btn) {
    btn.addEventListener("click", function () {
      selectNode(btn.getAttribute("data-node-id"));
    });
  });

  // --- EN/ES toggle: swaps all visible text in place, instantly ---
  function applyLanguage() {
    updateCaption(selectedId != null ? nodesById[selectedId] : null);
    if (selectedId != null) { renderDetail(nodesById[selectedId]); }
    topicNodes.forEach(function (tn) {
      var tg = groupFor(tn.id);
      if (tg) { tg.setAttribute("aria-label", tn["title_" + currentLang]); }
    });
    langToggle.textContent = currentLang === "en" ? "EN / ES → ES" : "ES / EN → EN";
    Array.prototype.forEach.call(colophonLinks, function (btn) {
      btn.textContent = btn.getAttribute("data-title-" + currentLang);
    });
    renderHistoryPanel();
  }
  langToggle.addEventListener("click", function () {
    currentLang = currentLang === "en" ? "es" : "en";
    applyLanguage();
  });

  // --- Dark/light theme toggle: overrides prefers-color-scheme via
  // documentElement[data-theme], which CSS already defines at matching
  // specificity (render_css_variables) -- background/color/fill/stroke
  // all carry a 180-200ms transition, so this reads as a smooth cross-
  // fade rather than a snap.
  //
  // The chosen theme persists across reopening this same exported file
  // via localStorage (file:// origins persist it per-path in Chromium/
  // Firefox, which matches the real use case here -- no server needed).
  // Reads/writes are wrapped in try/catch: some browser privacy modes
  // throw on localStorage access rather than just returning null, and a
  // reader's theme preference isn't worth a page-load error over. A
  // stored preference needs data-theme set explicitly on load (not just
  // inside the click handler, which is all that existed before) -- the
  // page otherwise relies purely on the prefers-color-scheme media query
  // until the first click, which would silently ignore anything stored. ---
  var themeOrder = ["dark", "light"];
  var storedTheme = null;
  try { storedTheme = localStorage.getItem("mae-guide-theme"); } catch (e) { /* ignore */ }
  var themeIdx = themeOrder.indexOf(storedTheme);
  if (themeIdx === -1) {
    themeIdx = (window.matchMedia && window.matchMedia("(prefers-color-scheme: light)").matches) ? 1 : 0;
  } else {
    document.documentElement.setAttribute("data-theme", themeOrder[themeIdx]);
  }
  themeToggle.addEventListener("click", function () {
    themeIdx = (themeIdx + 1) % themeOrder.length;
    document.documentElement.setAttribute("data-theme", themeOrder[themeIdx]);
    try { localStorage.setItem("mae-guide-theme", themeOrder[themeIdx]); } catch (e) { /* ignore */ }
  });

  // Only call setSidebarOpen() if a preference was actually stored --
  // leaving data-sidebar unset otherwise means the plain per-breakpoint
  // CSS default (open on desktop, closed on mobile) applies with zero
  // flash for a first-ever visit. A returning visitor with an explicit
  // stored value still gets a brief flash-then-correct, same tradeoff the
  // theme preference above already accepts (this script runs at the end
  // of <body>, not a synchronous anti-FOUC <head> script).
  var storedSidebarCollapsed = null;
  try { storedSidebarCollapsed = localStorage.getItem("mae-guide-sidebar-collapsed"); } catch (e) { /* ignore */ }
  if (storedSidebarCollapsed === "true") {
    setSidebarOpen(false);
  } else if (storedSidebarCollapsed === "false") {
    setSidebarOpen(true);
  }

  applyLanguage();
  // Resume the reader's last-open node (user request), falling back to
  // the anchor/spine node the same way this always worked before --
  // matching "Home" as a real default rather than an empty-state page
  // when there's no stored node, or it no longer exists in this export
  // (e.g. a stale value from a differently-scoped export at the same
  // path, or a node that was pruned). Uses replaceState, not selectNode's
  // pushState, so the page's very first load establishes the starting
  // history entry instead of creating a second one under it -- Back from
  // the first real navigation should leave the page, not land on an
  // invisible duplicate of itself.
  var storedLastNode = null;
  try { storedLastNode = localStorage.getItem("mae-guide-last-node"); } catch (e) { /* ignore */ }
  var initialNodeId = (storedLastNode && nodesById[storedLastNode]) ? storedLastNode : anchorId;
  applySelection(initialNodeId);
  // Keep Previous/Next's position consistent with wherever the restored
  // node actually lands in the reading order -- same resync the popstate
  // handler above already does for Back/Forward, needed here for the same
  // reason: walkIndex otherwise defaults to the anchor's own position
  // regardless of what was actually just selected.
  var initialWalkIdx = readingOrder.indexOf(initialNodeId);
  if (initialWalkIdx !== -1) { walkIndex = initialWalkIdx; }
  updateWalkButtons();
  try {
    history.replaceState({ nodeId: initialNodeId }, "", '#' + initialNodeId);
  } catch (e) { /* ignore -- see the try/catch in selectNode() above */ }
})();
