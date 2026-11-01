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
}

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
  nodes: {
    id: string;
    /**
     * The node's anchor id on this page (`id-…`, `cid-…`, or a heading's
     * `h-<slug>`; a slug keeps letters of any script, lower-cased, runs of
     * anything else one `-`, so it may be non-ASCII: `h-глава-2`; a
     * repeated title's heading is `h-<slug>--2`, `--3`, …). Null for the
     * file node. Percent-encode it to put it in another page's URL.
     */
    anchor: string | null;
    title: T;
    /** The title with links replaced by what they show: no <a> inside, to wrap in a link. */
    title_unlinked: T;
    aliases: T[];
    properties: { key: string; value: T }[];
  }[];
  diagnostics: { kind: string; start: number; end: number }[];
}

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

export type Op = 'semantics' | 'renderHast' | 'renderHtml' | 'text' | 'classifyLink' | 'version';

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
  callRaw(op: Op, request: string): string;
  call(op: Op, request?: Request): unknown;
  semantics(request: Request): SemanticsResult;
  renderHast(request: Request): Rendered<HastRoot>;
  renderHtml(request: Request): Rendered<string>;
  text(request: Request): string;
  classifyLink(request: Request): LinkClass;
  version(): { api: string; crate: string };
  liveBytes(): number;
}

/** Instantiates the module synchronously from its bytes (or a compiled module). */
export declare function instantiate(bytes: BufferSource | WebAssembly.Module): MaeOrg;
