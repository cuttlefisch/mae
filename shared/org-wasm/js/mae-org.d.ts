// Types for mae-org.mjs. SPDX-License-Identifier: GPL-3.0-or-later
//
// Spans are UTF-8 BYTE offsets into the request's text, not UTF-16 indices.

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
  /** KB node id -> where it renders. An unresolved link shows its description or `placeholder`. */
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
    anchor: string | null;
    title: T;
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
 * One module instance. Not re-entrant; calls are synchronous. If a call
 * traps (WebAssembly.RuntimeError), the instance throws on every later
 * call: instantiate a new one.
 */
export declare class MaeOrg {
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
