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
  | 'other-issuer' | 'other-subject' | 'expired'
  // backup refusals ('too-long' as above)
  | 'not-an-envelope' | 'unknown-version' | 'unknown-kdf' | 'kdf-params' | 'passphrase' | 'open'
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
  /** The challenge text, or the daemon's whole `/identity/challenge` answer. */
  challenge: string;
  /** Unix seconds; default this host's clock. */
  now?: number;
}

export interface SignedChallenge {
  challenge: Challenge;
  /** The body `POST /identity/bind` takes. */
  body: { challenge_id: string; public_key: B64; signature: B64 };
}

/** Argon2id costs: memory in KiB (19456..=262144), passes (2..=10), lanes (1..=4). */
export interface KdfParams { m: number; t: number; p: number }

export interface LoadOptions {
  /** Replaces crypto.getRandomValues as the entropy source. Tests only. */
  randomFill?: (view: Uint8Array) => void;
}

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

  /** The key's seed, sealed under the passphrase (NFC-normalised here): envelope v1 text. */
  backupSeal(request: { key: KeyHandle; passphrase: string; kdf?: KdfParams }): string;
  /** Opens an envelope ('open' for a wrong passphrase or any alteration); holds the key. */
  backupOpen(request: { envelope: string; passphrase: string }): KeyInfo;
  /** The envelope's checked KDF costs, without opening it. */
  backupParams(envelope: string): KdfParams;

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
}
