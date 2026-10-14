# Architecture and security model

## Components

```
┌──────────────── Tauri app ────────────────┐          ┌──────── Server ────────┐
│ React + Mantine UI  (app/src)             │          │ Go HTTP + WebSocket     │
│        │ invoke / events                  │  HTTPS   │   ├─ MySQL (accounts,   │
│ Tauri shell (app/src-tauri)               │ ───────► │   │  keys, queues,      │
│        │                                  │   WSS    │   │  groups, vault)     │
│ chat-core (crates/chat-core)              │          │   └─ MinIO (encrypted   │
│   libsignal · SQLCipher · sync · media    │          │      attachments)       │
└───────────────────────────────────────────┘          └─────────────────────────┘
```

`chat-core` owns all logic: protocol, storage, networking, sync. It runs on a dedicated
thread (libsignal's store traits are `!Send`) and is driven through `Core::run`. The UI is a
pure view that calls commands (`app/src/lib/backend.ts`) and listens to events. The same core
will be reused by the mobile apps (Tauri mobile).

## Accounts and keys

```
recovery phrase (BIP39, 24 words) = 256-bit account secret
  ├─ HKDF "identity" → Signal identity key pair (Curve25519) — stable for the account
  ├─ HKDF "recovery" → Ed25519 key; its public key is registered on the server
  ├─ HKDF "vault"    → AES-256-GCM key for the encrypted vault
  └─ HKDF "profile"  → profile key (encrypts name/about/avatar)

password → Argon2id(salt, 64 MiB, t=3) → auth_key (sent) ‖ wrap_key (never sent)
  wrapped_account_key = AES-256-GCM(wrap_key, account secret)   (stored on the server)
```

* **Registration:** the client generates the phrase and all keys, the server assigns a random
  16-digit number. The server stores `sha256(auth_key)`, the wrapped account key, the public
  identity key and the recovery public key.
* **Login on a new device:** pre-login returns the KDF salt (a deterministic fake one for
  unknown numbers), login with `auth_key` returns the wrapped key; the device unwraps the
  account secret locally and derives the identity. Each device has its own registration id,
  signed pre-key, Kyber pre-keys and one-time pre-keys, all signed by the account identity.
  A server cannot add a device that others would talk to, because it cannot sign pre-keys.
* **Recovery:** the client signs a server challenge with the recovery key derived from the
  phrase, and sets a new password. All other devices are logged out.
* **Password change** re-wraps the same secret; nothing else changes.

## Messaging

* 1:1: every message is encrypted separately for every device of the recipient and for the
  sender's other devices (sync copy). The server checks that the device list is complete
  (409 missing/extra, 410 stale) so clients learn about new devices immediately.
* Groups: membership is kept on the server (needed for fan-out and permissions) but the name,
  description and avatar key are encrypted with a group key shared pairwise. Messages use
  Sender Keys: one encryption per message, distributed once per device via 1:1 sessions.
  When a member leaves or is removed, every sender starts a new sender-key chain, and the
  removing admin rotates the group key.
* Group invite links (`engine/invites.rs`): the link carries a 16-byte secret the server
  never sees. From it the clients derive a token (the server stores its hash) and a key for
  an encrypted preview (name, description, and the group key when no approval is needed).
  With approval, the request carries the requester's name and profile key encrypted with the
  same key, and the approving admin sends the group key pairwise. The secret is kept in the
  encrypted group metadata so every admin can show the link; removing a member replaces it.
* Deleting an active group chat only clears its history on this device (the group stays);
  only admins can clear the history for every member (an end-to-end `GroupHistoryCleared`
  message that receivers accept only from an admin).
* @mentions are UTF-16 ranges inside the message; text formatting (`*bold*`, `_italic_`,
  `~strike~`, `` `code` ``) is display-only, with the same parser on desktop and mobile.
* Payloads are JSON inside the Signal ciphertext, padded to 160-byte blocks.
* Delivery / read receipts, typing, edits, deletes and reactions are all end-to-end
  encrypted messages; the server only sees opaque envelopes.
* Decryption failures trigger a resend request (the sender re-sends the same message id,
  receivers de-duplicate).
* Attachments: AES-256-CBC + HMAC-SHA256 with a random key per file, uploaded to MinIO through
  the server; the key and digest travel inside the encrypted message.

## Multi-device sync (vault)

Contacts (local names, blocks, profile keys), group keys, conversation settings and, if
enabled, message history are stored on the server as records encrypted with the vault key.
Record ids are HMACs, so the server cannot link records to conversations. A newly linked
device downloads the vault and gets the history (media stays downloadable while the server
keeps it, 30 days by default). Disappearing messages are never written to the vault.

## Local storage

* SQLCipher database (`chat.db`), encrypted with a random 32-byte key.
* Once logged in, that key is locked with the **account password**: `chat.db.lock` holds it
  sealed (AES-256-GCM) with `Argon2id("chatapp/local-db\0" || password, local salt)`. The
  salt is random, generated on the device and never sent anywhere, and the domain prefix
  differs from the server derivation, so nothing the server stores helps to open the file.
  At start the engine runs on an empty in-memory database until `unlock(password)`; without
  the password the files on disk cannot be decrypted. Changing the password only re-wraps
  the key. A password changed on another device is detected (`GET /v1/account` returns the
  current kdf) and the app asks for the new one (`protect_local_data`). Forgotten password:
  `reset_local_data` deletes the local data; history comes back from the encrypted vault
  after login or recovery.
* Before the first login (and right after linking a device, until the password is entered)
  the key lives in the OS keystore (Keychain, Credential Manager, Secret Service) with a
  user-only key file as fallback.
* **Several accounts on one device (desktop and mobile).** Each account (profile) has its own data
  directory, `<data dir>/profiles/<random id>/` (an account from before profiles stays in the
  data directory itself). `profiles.json` lists them for the start screen with only a label,
  the last 4 digits and the server host (nothing for profiles hidden from the start screen),
  plus a salted Argon2id hash of the account number to open hidden ones by their full number;
  never a password or key. One profile is active per process: the app starts on a blank
  profile (the start screen), and opening a profile starts a core on its directory, unlocks it
  with the password, swaps it in and shuts the previous core down (`crates/chat-core/src/
  profiles.rs`, shared by the Tauri shell and the mobile bridge, which routes the profile
  commands through its generic `invoke`). On phones, the account of a version before profiles
  stays in the app data directory as the profile "default" (nothing is moved). Only the active
  profile is connected, so messages, calls and local notifications come only from it; the other
  accounts stay logged in but offline (their messages wait on the server). Push for inactive
  profiles is not handled: a push token registered by an account stays with its server, so a
  push may still show the generic "New message" notification without saying for which account.
  "Switch account" keeps the account and its data (locked); "Log out" unlinks this device and
  deletes its data, and warns first when it is the account's only device.
* **Media files** (attachments, voice notes, avatars) are **encrypted at rest** in `media/` (and
  `media-temp/` in temporary mode).
  - **Format:** 64 KiB AES-256-GCM chunks; see `media_crypto.rs`.
  - **Key:** a random media key stored in the password-locked database.
  - **Reading:** the UI asks the core for the decrypted bytes (only the requested range for
    audio/video seeking).
  - **Exceptions:** the user's own "save as", and, on mobile, short-lived copies for the share
    sheet and the audio player, kept in the private cache and removed after use and at start.
  - **Deletion:** a message's files are deleted with it (expiry, deletion, wiping), unless a
    forwarded copy still uses them. A sweep every 10 minutes deletes files nothing references.
    Files from older versions are encrypted at the first start.
* Temporary mode keeps conversations, messages, reactions, receipts, the outbox and pins/stars
  in SQLite TEMP tables (memory only) and deletes their media on exit and at start.
* Pinned (24 h / 7 days / 30 days, at most 3 per chat) and starred (7 days) messages are kept
  on this device only, never sent or backed up; they expire on their own and go with their
  message.

## What the server knows

Account number, creation time, devices (name, platform, last seen), public keys, group
membership, the recipient and time of every message (for 1:1 messages between contacts, not
the sender: see "Sealed sender"), who sends to which group, attachment sizes, the
optional verified phone number (and its hash for discovery), IP addresses. It does not know
message contents, names, avatars, group names, contacts' local names or passwords.

## Identity pinning (malicious server / interception)

An account's Signal identity key is derived from its recovery phrase: it never changes (not
on password change, not on recovery) and every device of the account uses it. Clients
therefore pin it on first contact and refuse any device that presents another key, instead
of Signal's default "trust the new key and show a notice":

* sending to such an account fails (`untrusted_identity`, no retries) and the conversation
  shows a red warning; messages from such a device are dropped;
* our own account's devices must present our own identity key, so a device injected into
  our account receives nothing (not even sync messages);
* pins travel with the contact records of the encrypted vault, so a newly linked device does
  not have to trust the server for contacts it already knows.

So a compromised server (or someone who intercepted a login over plain HTTP) cannot read
messages by adding a device; it can only block delivery. First contact is trust-on-first-use;
comparing safety numbers in person removes that last gap. Covered by
`crates/chat-core/tests/evil_server.rs` (identity swap and device injection with a fully
valid attacker bundle).

Over plain HTTP an observer still sees metadata (who talks to whom, when, sizes, IPs) and
the device token (which allows deleting queued messages, not reading them): production
servers belong behind HTTPS.

## Sealed sender

1:1 messages hide the sender from the server, as in Signal:

* The server signs a 24-hour **sender certificate** for each device (account id, device id,
  identity key) with a key certified by its **trust root** (XEdDSA, libsignal's
  `sealed_sender.proto` format; `GET /v1/certificate/delivery`). The trust root is published
  in `/v1/info`.
* The sender wraps the normal Signal message with libsignal's sealed sender v1: the sender's
  identity and certificate are encrypted to the recipient's identity key. The envelope
  (type 6) is delivered through `PUT /v1/sealed/messages/{account}` **without** the sender's
  token, on a separate connection pool, so the server stores it with no source.
* To limit abuse, delivery requires the recipient's **unidentified access key**
  (HKDF of the recipient's profile key, 16 bytes), which only their contacts know. Pre-key
  bundles can be fetched the same way (`GET /v1/sealed/keys/...`), so starting a session does
  not reveal the pair either. A refused key (or no profile key yet) falls back to identified
  delivery.
* The recipient checks the certificate against the trust root, that the certificate's identity
  key is the key that sealed the message (libsignal), and then decrypts the inner message as
  usual, so identity pinning still applies.

## Known limitations / roadmap

* Sealed sender covers 1:1 messages between contacts that know each other's profile key.
  Group messages, messages to our own devices and the first message to a stranger are still
  identified. The server also sees the sending IP address and timing.
* Contact discovery by phone uses plain SHA-256 hashes; phone numbers are low-entropy, so a
  malicious server could reverse them. It is optional and off by default.
* Video calls and group calls are not implemented (1:1 voice calls are, see API section 16).
* The server runs as a single instance (WebSocket routing and presence are in-process).
