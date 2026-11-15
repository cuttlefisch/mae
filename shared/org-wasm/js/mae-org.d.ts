// Types for mae-org.mjs. SPDX-License-Identifier: GPL-3.0-or-later
//
// Every span (`start`/`end`, in results and diagnostics) is a UTF-8 BYTE
// offset into the request's `text`, NOT a JavaScript string index (UTF-16
// code units). Slice `new TextEncoder().encode(text)`, not `text`.

export interface Request {
  text?: string;
  parse?: {
    todo?: { todo: string[]; done: string[] }[];
    max_depth?: number;
    link_types?: string[];
  };
  semantics?: {
    exclude_tags?: string[];
    select_tags?: string[];
    archive_tag?: string;
    archived_trees?: 'headline' | 'all' | 'none';
    link_types?: { web?: string[]; mail?: string[]; other?: string[] };
    link_exclude_keywords?: string[];
    link_exclude_properties?: string[];
  };
  render?: { headline_ids?: 'explicit' | 'slug'; images?: boolean; property_drawers?: boolean };
  /**
   * KB node id -> where it renders. An unresolved link shows its description or `placeholder`.
   * An `href` must be an http(s)/mailto URL or a scheme-less reference, already URL-encoded;
   * one holding a character reference (`&#…`, `&name;`) is refused like any unsafe href.
   */
  resolver?: { targets?: Record<string, { href: string; title?: string | null }>; placeholder?: string };
  /** For `text`: the KB node whose title and body to read. */
  node?: string;
  /** For `classifyLink`: the link path. */
  path?: string;
  /** For `syntaxSection`: one entry of a `syntax` result's `sections`, unchanged. */
  section?: SyntaxSectionInfo;
  /**
   * For `syntax`/`syntaxSection`: whether a span's `end` includes the node's post-blank
   * (the spaces after an object, the blank lines after an element). Default true, as
   * `org-element` reports it; false ends `*bold*  ` after the closing `*`.
   */
  post_blank?: boolean;
  /** For `syntaxSection`: the `file` object of the same `syntax` result, unchanged. */
  file?: SyntaxFile;
  /** For the CUSTOM_ID ops and `headlineProperties`: a headline is in scope when its headline line starts in one (`start <= at < end`). Absent: all. */
  ranges?: [number, number][];
  /** For `customIdEdits`: lowercase UUID v4s, used in document order; too few throws, extras are unused. */
  uuids?: string[];
  /** For `replaceCustomId`: the start of a CUSTOM_ID value, as `duplicateCustomIds` gives it. */
  position?: number;
  /** For `replaceCustomId`: the lowercase UUID v4 to write. */
  uuid?: string;
}

/** `text[start..end]` (UTF-8 bytes) becomes `insert`. */
export interface TextEdit { start: number; end: number; insert: string }

/** One headline's CUSTOM_ID: its value, and its headline line (no line ending). */
export interface CustomIdOccurrence { value: [number, number]; heading: [number, number] }

/** One property line: `key` canonical (ASCII upper case; an appending key keeps its `+`), `keyAsWritten` as in the file, `value` trimmed as Org reads it. */
export interface HeadlineProperty { key: string; keyAsWritten: string; value: string }

/** A headline with a property drawer Org reads: its headline line (no line ending), level (stars) and every property line in file order, repeated keys included (Org reads the last). */
export interface HeadlineProperties { heading: [number, number]; level: number; properties: HeadlineProperty[] }

/** File facts a section's re-highlight depends on, as `syntax` returns them. */
export interface SyntaxFile {
  line_ending: 'lf' | 'crlf' | 'cr';
  radio_targets: string[];
  todo: { todo: string[]; done: string[] }[];
}

/**
 * A region `syntaxSection` can re-highlight alone: the text before the first
 * headline, or one headline's own section (after its line, up to its first
 * child headline). Byte offsets into the whole text.
 */
export interface SyntaxSectionInfo {
  start: number;
  end: number;
  level: number;
  ancestors: number;
  declarations: number;
  followed: boolean;
  /**
   * The region's line-ending facts (bits 0-2) and the terminator before it
   * (bits 3-4). Opaque: pass it back to `syntaxSection` unchanged. Required
   * since 0.5.0.
   */
  eol: number;
}

/**
 * Highlighting spans: `spans` is flat, three numbers a span —
 * `[kindIndex, start, end, …]`, `kindIndex` into `kinds` (SyntaxKind names
 * such as "HEADLINE", "BOLD"). UTF-8 byte offsets into the WHOLE text.
 * Plain text, spaces, line endings and blank lines are not listed. A span's
 * end includes its post-blank unless the request sets `post_blank: false`.
 */
export interface SyntaxSpans {
  kinds: string[];
  spans: number[];
}

export interface SyntaxResult extends SyntaxSpans {
  file: SyntaxFile;
  sections: SyntaxSectionInfo[];
}

/**
 * `needs_full`: the edit changed what the rest of the file reads (a headline
 * line, a radio target or TODO keyword line, the line ending before the next
 * headline, or the file's line-ending convention); send the whole text to
 * `syntax` again.
 */
export type SyntaxSectionResult =
  | SyntaxSpans
  | { needs_full: 'headline' | 'declarations' | 'boundary' | 'line_ending' };

export interface Span { start: number; end: number }

/** A hast tree (unified/rehype). */
export interface HastText { type: 'text'; value: string }
export interface HastElement {
  type: 'element';
  tagName: string;
  properties: Record<string, string | string[]>;
  children: (HastElement | HastText)[];
}
export interface HastRoot { type: 'root'; children: (HastElement | HastText)[] }

export interface Rendered<T> {
  body: T;
  /**
   * One per KB node the page EXPORTS (none for a node in a COMMENT or
   * `:noexport:` subtree, as written or as its macros expanded; a headline a
   * macro made is never one). What to publish is these, not `semantics`.
   */
  nodes: {
    id: string;
    /**
     * The node's anchor id on this page (`id-…`, `cid-…`, or a heading's
     * `h-<slug>`; a slug keeps letters of any script, lower-cased, runs of
     * anything else one `-`, so it may be non-ASCII: `h-глава-2`; a
     * repeated title's heading is `h-<slug>--2`, `--3`, …; titles that
     * differ only in case, punctuation or blanks share a slug, so `A b` and
     * `A-b` are `h-a-b` and `h-a-b--2`; a link in a title adds only its
     * description). Null for the
     * file node. Percent-encode it to put it in another page's URL. The
     * page's other anchors: `t-N` for its N-th `<<target>>` and `r-N` for
     * its N-th radio target (opaque: no id spells a target's name), `fn-N`
     * and `fnr-N` for footnotes.
     */
    anchor: string | null;
    title: T;
    /** The title with links replaced by what they show: no <a> inside, to wrap in a link. */
    title_unlinked: T;
    aliases: T[];
    properties: { key: string; value: T }[];
  }[];
  /**
   * Spans in the request's `text` (the source), even where macros expanded.
   * `kind` is one of:
   * - `too_deep`, `unparsed`: content left out (nested past the renderer's
   *   depth, or text the parser left opaque);
   * - `macro_undefined`, `macro_circular`, `macro_eval`, `macro_unsupported`,
   *   `macro_budget`: the document's macros could not be expanded as Org's
   *   export expands them (an undefined macro, a circular expansion, an
   *   `(eval …)` template, a value MAE cannot know from the text, or an
   *   expansion past a bound linear in the source). As Org's export then
   *   produces nothing, `body` and `nodes` are EMPTY;
   * - `macro_environment`: a `{{{time}}}`/`{{{modification-time}}}` macro
   *   expanded with a fixed time (1970-01-01 00:00:00 UTC), or an
   *   `{{{input-file}}}` with nothing (informational);
   * - `fragment_macro`: a KB link's `#fragment` holds a macro and names a
   *   heading on another page, whose macros this render cannot know: the
   *   link goes to the node without the fragment;
   * - `footnote_undefined`: a footnote reference whose label has no
   *   definition Org's export can use (one only in a COMMENT subtree, a
   *   comment block or comment lines, or none at all; a definition in a
   *   `:noexport:`, excluded, archived or unselected subtree or a drawer is
   *   still used); it renders nothing. The label is
   *   the source text at the span. Org's export fails there; refuse such a
   *   document if that matters to you;
   * - `link_attribute`: a KB link's `rel=`, `w=` or `c=` that is not a
   *   valid value (a relation name `[a-z][a-z_-]{0,63}`: no digit; a finite
   *   number); its `data-org-*` attribute is left out;
   * - `property_withheld`: a property value whose macros cannot be expanded
   *   (as `{{{property(KEY)}}}` would expand them at its entry): it renders
   *   as nothing in `nodes` and in the drawer. The span is the drawer's.
   */
  diagnostics: { kind: string; start: number; end: number }[];
}

/**
 * The KB projection. Its node titles, aliases, property values, link
 * descriptions and link fragments are the SOURCE as plain text: an inline src block, a babel
 * call or an export snippet the export hides is spelled out in them, and macros are not
 * expanded (a render expands them as Org's export does). Never
 * show them to a reader: display text comes from a render's `nodes`
 * (`title`, `title_unlinked`, `aliases`, `properties`) or from `text`.
 */
export interface SemanticsResult {
  semantics: {
    file_tags: string[];
    nodes: {
      id: string; origin: 'file' | 'headline' | 'item'; level: number; kind: string | null;
      title: string | null; aliases: string[]; own_tags: string[]; tags: string[];
      properties: { key: string; value: string }[]; todo: string | null; priority: string | null;
      span: Span; body_span: Span;
    }[];
    links: {
      from: string | null; kind: string; path: string; target: string; fragment: string | null;
      search: string | null; rel: string; weight: number; confidence: number;
      attrs: { key: string; value: string }[]; description: string | null; span: Span;
    }[];
    transclusions: { from: string | null; member: string; role: string; args: string | null; span: Span }[];
    diagnostics: ({ kind: string; span: Span } & Record<string, unknown>)[];
    unrendered: { span: Span; element: string; region: string }[];
    footnotes: { label: string; span: Span }[];
  };
  diagnostics: { kind: string; start: number; end: number }[];
  line_ending: 'lf' | 'crlf' | 'cr';
}

export interface LinkClass {
  kind: string; target: string; fragment: string | null; search: string | null; rel: string;
  weight: number; confidence: number; attrs: { key: string; value: string }[];
  description: string | null;
}

export type Op =
  | 'semantics' | 'renderHast' | 'renderHtml' | 'text' | 'classifyLink' | 'version'
  | 'syntax' | 'syntaxSection'
  | 'headingsMissingCustomId' | 'customIdEdits' | 'duplicateCustomIds' | 'replaceCustomId'
  | 'headlineProperties';

/**
 * The module, one instance at a time. Not re-entrant; calls are
 * synchronous. A call that throws for ANY reason (a trap, the host stack
 * exhausted inside the module, which V8 reports as a RangeError, or
 * anything else) rethrows, and retires the instance it ran in: its memory
 * is not trusted again. The next call runs in a fresh instance of the same
 * compiled module (`restarts` counts them). A `MaeOrg` constructed from a
 * bare instance, without its module, cannot restart: every call after a
 * failed one throws.
 */
export declare class MaeOrg {
  constructor(instance: WebAssembly.Instance, module?: WebAssembly.Module | null);
  /** Instances replaced after a failed call. */
  readonly restarts: number;
  /** Throws a TypeError for a request that is not a string, and an Error for an unknown `op`, before touching the instance. */
  callRaw(op: Op, request: string): string;
  call(op: Op, request?: Request): unknown;
  /** KB data, not display text: see SemanticsResult. */
  semantics(request: Request): SemanticsResult;
  renderHast(request: Request): Rendered<HastRoot>;
  renderHtml(request: Request): Rendered<string>;
  /** Search text, macros expanded as a render expands them: empty when they cannot be (see `Rendered.diagnostics`). */
  text(request: Request): string;
  classifyLink(request: Request): LinkClass;
  version(): { api: string; crate: string };
  /** Spans for the whole `text`, plus the regions `syntaxSection` can redo alone. */
  syntax(request: Request): SyntaxResult;
  /** Spans for one region: `text` is the region's NEW text, `section`/`file` as `syntax` gave them. */
  syntaxSection(request: Request): SyntaxSectionResult;
  /** The headline lines needing a CUSTOM_ID: no non-empty CUSTOM_ID, and no non-empty ID (a KB node). */
  headingsMissingCustomId(request: Request): { headings: [number, number][] };
  /** Edits giving each headline in scope a CUSTOM_ID, placed as `org-entry-put` places it; sorted descending. */
  customIdEdits(request: Request): { edits: TextEdit[] };
  /** CUSTOM_IDs several headlines share, grouped in document order. Never chooses which to keep. */
  duplicateCustomIds(request: Request): { groups: CustomIdOccurrence[][] };
  /** The edit replacing the CUSTOM_ID value that starts at `position` with `uuid`. */
  replaceCustomId(request: Request): { edit: TextEdit };
  /** Every headline with a property drawer Org reads, node or not, in document order. */
  headlineProperties(request: Request): { headlines: HeadlineProperties[] };
  liveBytes(): number;
}

/** Instantiates the module synchronously from its bytes (or a compiled module). */
export declare function instantiate(bytes: BufferSource | WebAssembly.Module): MaeOrg;
