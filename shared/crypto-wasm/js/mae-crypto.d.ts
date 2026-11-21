// Types for mae-crypto.mjs. SPDX-License-Identifier: GPL-3.0-or-later
//
// Bytes are standard base64 with padding (RFC 4648 §4) everywhere. A member
// key is named by the handle (`key`) the instance gave it; handles are
// per instance and never reused within one. The contract is documented in
// docs/E2E_ENCRYPTION.md, "Browser key material".

/** Base64 (standard alphabet, padded). */
export type B64 = string;
/** A member-key handle, valid only in the instance that issued it. */
export type KeyHandle = number;

export type ErrorCode =
  | 'bad-request' | 'unknown-op' | 'no-key' | 'keystore-full' | 'no-entropy' | 'bad-public-key'
  // identity-binding challenge refusals
  | 'too-long' | 'not-a-challenge' | 'wrong-domain' | 'empty-field' | 'other-key'
  | 'other-issuer' | 'other-subject' | 'other-audience' | 'expired' | 'expiry-too-far'
  // backup refusals ('too-long' as above)
  | 'not-an-envelope' | 'unknown-version' | 'unknown-kdf' | 'kdf-params' | 'passphrase'
  | 'passphrase-weak' | 'open' | 'fingerprint-mismatch'
  // content crypto
  | 'decrypt' | 'malformed' | 'bad-key';

export class MaeCryptoError extends Error {
  readonly op: string;
  readonly code: ErrorCode;
}

export interface Version {
  /** The contract version (`mae-crypto-vX.Y.Z` tags). */
  version: string;
  /** The backup envelope version written and read: 1. */
  backup_version: number;
  /** The challenge domain tag: "mae-identity-binding-v1". */
  challenge_domain: string;
}

export interface KeyInfo {
  key: KeyHandle;
  /** The Ed25519 public key (32 bytes). */
  public_key: B64;
  /** `SHA256:` + unpadded base64 of SHA-256(public key). */
  fingerprint: string;
  /** The X25519 content-key wrap public key (ADR-041), 32 bytes. */
  wrap_public: B64;
}

export interface Challenge {
  domain: string;
  iss: string;
  sub: string;
  aud: string;
  fingerprint: string;
  nonce: string;
  expires_at: number;
}

export interface ChallengeRequest {
  key: KeyHandle;
  /** The OIDC issuer and subject the key holder states as their own. */
  issuer: string;
  subject: string;
  /** The daemon the key holder means to bind at: its canonical resource URI (the challenge's `aud`). */
  audience: string;
  /** The challenge text, or the daemon's whole `/identity/challenge` answer. */
  challenge: string;
  /** Unix seconds; default this host's clock. Refused if expires_at is past it or more than 3600 s ahead. */
  now?: number;
}

export interface SignedChallenge {
  challenge: Challenge;
  /** The body `POST /identity/bind` takes. */
  body: { challenge_id: string; public_key: B64; signature: B64 };
}

/** Argon2id costs: memory in KiB (19456..=262144), passes (2..=10), lanes (1..=4). */
export interface KdfParams { m: number; t: number; p: number }

/** What backupParams reads from an envelope without opening it. */
export interface BackupHeader extends KdfParams {
  /** The fingerprint of the key the envelope holds. */
  fingerprint: string;
}

/**
 * TEST ONLY. Never set either option in production: `randomFill` replaces
 * the entropy source every key, salt and nonce comes from, and is refused
 * unless `dangerouslyReplaceEntropyForTests` is `true` beside it.
 */
export interface LoadOptions {
  randomFill?: (view: Uint8Array) => void;
  dangerouslyReplaceEntropyForTests?: true;
}

/** The largest request (UTF-8 bytes) the loader passes on: 15,029,592, the daemon's default max_message_bytes. */
export const MAX_REQUEST_BYTES: number;

export function instantiate(bytes: BufferSource | WebAssembly.Module, options?: LoadOptions): MaeCrypto;

export class MaeCrypto {
  /** Runs an operation by name on a JSON request string; returns the response string. */
  callRaw(op: string, request: string): string;
  /** Runs an operation by name on a request object; returns its result or throws MaeCryptoError. */
  call(op: string, request?: object): unknown;

  version(): Version;

  /** A new member key from 32 bytes of the host's randomness ('no-entropy' without it). */
  generate(): KeyInfo;
  /** Holds the key with this 32-byte seed (e.g. one exportSeed gave). */
  importSeed(seed: B64): KeyInfo;
  /** The 32-byte seed: the key's whole secret. Only for an explicit export. */
  exportSeed(key: KeyHandle): B64;
  /** Wipes the key; true if the handle named one. */
  forget(key: KeyHandle): boolean;
  keyInfo(key: KeyHandle): KeyInfo;
  /** The fingerprint of any Ed25519 public key ('bad-public-key' if it is not one). */
  fingerprint(publicKey: B64): string;

  checkChallenge(request: ChallengeRequest): Challenge;
  signChallenge(request: ChallengeRequest): SignedChallenge;

  /**
   * Signs a KB content op (ADR-036 F5) with a held key. The module builds the signed message itself
   * ('maecontent/v1'); the header's author is the key's own fingerprint. Send the result as the top-level
   * params of `kb/node_update` (beside kb_id, node_id, update), or as `params.content_header` of
   * `sync/update`. Refusals: 'empty-field', 'nul-in-field', 'bad-request' (incl. a non-canonical epoch),
   * 'no-key'.
   */
  signOp(request: SignOpRequest): ContentOpHeader;

  /**
   * The key's seed, sealed under the passphrase: envelope v1 text. The module NFC-normalises the
   * passphrase; it must then have at least 12 characters ('passphrase-weak').
   */
  backupSeal(request: { key: KeyHandle; passphrase: string; kdf?: KdfParams }): string;
  /**
   * Opens an envelope ('open' for a wrong passphrase or any alteration, 'fingerprint-mismatch' if the
   * key is not the one the header names); holds the key. No length floor on opening.
   */
  backupOpen(request: { envelope: string; passphrase: string }): KeyInfo;
  /** The envelope's checked header (fingerprint and KDF costs), without opening it. */
  backupParams(envelope: string): BackupHeader;

  /** A fresh 32-byte content key. */
  contentKeyGenerate(): B64;
  /** nonce(24) ‖ XChaCha20-Poly1305 ciphertext+tag. Traps (retiring the instance) without entropy. */
  contentSeal(contentKey: B64, plaintext: B64): B64;
  contentOpen(contentKey: B64, sealed: B64): B64;
  /** The content key sealed to a member's wrap_public. Traps without entropy. */
  contentWrap(contentKey: B64, wrapPublic: B64): B64;
  /** A content key wrapped to this held key's wrap_public. */
  contentUnwrap(key: KeyHandle, wrapped: B64): B64;

  /** Bytes allocated in the module and not freed. */
  liveBytes(): number;
  /** Zeroes the instance's memory (every held key) and refuses later calls. A failed call does the same. */
  dispose(): void;
}

export interface SignOpRequest {
  key: KeyHandle;
  kb_id: string;
  node_id: string;
  /** The author's yrs state vector at authoring time. */
  base_sv: B64;
  /** The author's epoch in the KB, as canonical decimal text: a random u64, so never a JS number. */
  epoch: string;
  /** The yrs update bytes the op carries. */
  update: B64;
  /** Unix seconds; defaults to this host's clock. */
  issued_at?: number;
}

/** The signed ADR-036 header, exactly as the daemon reads it. */
export interface ContentOpHeader {
  author: string;
  epoch: string;
  issued_at: number;
  base_sv: B64;
  sig: B64;
  author_pubkey: B64;
}

