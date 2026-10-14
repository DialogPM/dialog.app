# Key transparency — design

Scop: un server compromis **nu poate servi o cheie de identitate falsă** pentru contul X unor
utilizatori fără să fie detectat. Închide problema I8 din [SECURITY-REVIEW-CLIENT.md](SECURITY-REVIEW-CLIENT.md)
(primul contact avea încredere în directorul serverului) și punctul 4.9 din
[SIGNAL-GAP-ANALYSIS.md](SIGNAL-GAP-ANALYSIS.md). Doar `server-rs` publică jurnalul.

Design mai simplu decât al Signal (`libsignal-keytrans`: VRF + prefix tree + log de rădăcini + auditori
terți), potrivit modelului nostru în care **identitatea unui cont nu se rotește niciodată**:
un jurnal Merkle append-only (hash-uri RFC 6962, verificare RFC 9162) cu câte o intrare per cont.

## Server (`server-rs/src/keytrans.rs`, `api/keytrans.rs`, migrarea 0014)

- `kt_leaves(idx, account_id, identity_key, created_at)`: intrări în ordinea jurnalului, `idx` contiguu,
  niciodată modificate sau șterse (nici la ștergerea contului). `kt_state.size` serializează secvențiatorul.
- **Secvențiatorul** adaugă o intrare pentru orice cont fără una: imediat după înregistrare, la pornire
  (backfill pentru conturile existente) și la fiecare 30 s.
- Fiecare instanță ține în memorie toate hash-urile subarborilor perfecți (2n hash-uri) și se
  actualizează din MySQL înainte de fiecare răspuns; dacă ultima frunză cunoscută nu mai corespunde
  rândului din bază (restaurare/alterare), arborele e reconstruit — iar clienții detectează rescrierea.
- **Tree head semnat (STH)**: Ed25519 peste `"chatapp/kt/sth/v1" ‖ size ‖ root ‖ timestamp`, cheie derivată
  din `CHATAPP_SERVER_SECRET` (HMAC, ca celelalte chei ale serverului), publicată în `/v1/info` ca
  `kt_public_key`.
- Endpoint-uri publice, neautentificate (API.md §19): `head`, `proof/{account}`, `consistency`, `entries`.

## Client (`crates/chat-core/src/keytrans.rs` — verificări pure; `engine/keytrans.rs` — logica)

1. **Pin**: `kt_public_key` se fixează la prima conexiune; o cheie schimbată e refuzată (alertă).
2. **Primul contact**: înainte ca un pre-key bundle al unui cont nefixat să fie folosit
   (`send.rs::fetch_sessions` → `kt_require`), clientul cere dovada de incluziune; cheia din bundle
   trebuie să fie exact cheia din intrarea jurnalului, altfel trimiterea e refuzată (mesajul „eșuat”,
   avertisment `identity_mismatch` în conversație, alertă `key_mismatch`).
3. **Append-only**: se păstrează ultimul STH verificat; unul nou e acceptat doar cu dovadă de consistență
   validă (altfel `fork`); unul mai mic decât cel știut e `rollback`.
4. **Self-monitoring** (la 30 min și „Verifică acum”): clientul își verifică propria intrare și descarcă
   o singură dată tot jurnalul (pagini de 1000, compact range de O(log n) hash-uri), recalculează rădăcina
   și o compară cu STH-ul semnat. O intrare cu contul nostru și altă cheie → alertă `own_key_mismatch`
   (serverul a publicat o cheie falsă pentru noi, chiar dacă a creat o intrare validă pentru ea).
5. **Gossip**: fiecare mesaj de date poartă ultimul STH al expeditorului (`DataMessage.kt`, criptat E2E).
   Destinatarul îl compară cu al lui: aceeași mărime și altă rădăcină, sau nicio dovadă de consistență
   validă între ele → alertă `split_view`, cu ambele STH-uri semnate ca dovadă (nerepudiabilă).
6. **Verificare în fundal**: identitățile fixate din primul mesaj primit de la un cont nou sunt verificate
   în jurnal în cel mult un minut.

Starea stă în tabela `kv` (per dispozitiv). RPC-uri: `kt_status`, `kt_contact_status`, `kt_check_now`,
`kt_dismiss_alerts`; eveniment `kt_alerts`. UI: rândul „Transparența cheilor: verificată / neverificată
încă / ALERTĂ” pe ecranul numărului de siguranță și un banner global de alertă (desktop și mobil).

## Teste

- Unitare: `server-rs` (`keytrans::tests`: rădăcini față de definiția RFC 6962, dovezi verificate cu
  algoritmii RFC 9162, semnături) și `chat-core` (`keytrans::tests`: dovezi de incluziune/consistență
  inclusiv adversariale — rădăcină greșită, dovadă trunchiată/extinsă/goală, istoric rescris —, compact
  range, STH-uri, intrări false, split view).
- `crates/chat-core/tests/evil_server_kt.rs` (server real + acces direct la MySQL, ca atacator):
  cheie falsă fără intrare → refuzată; intrare falsificată → păcălește primul contact, dar proprietarul
  o găsește; jurnal rescris arătat altfel unui utilizator → expus prin gossip.

## Limitări

- **Fără auditori terți și fără VRF**: un server care partiționează perfect utilizatorii (nimeni din
  partea A nu vorbește vreodată cu cineva din partea B) nu e prins de gossip. Gossip-ul trece prin
  mesaje E2E, deci un MITM complet între exact două persoane îl poate filtra; e prins de oricare alt
  contact comun.
- **Detecția e ulterioară**, nu preventivă, când serverul falsifică și intrarea: primul contact e păcălit
  până când proprietarul își rulează self-check-ul (≤ 30 min online) și află; avertizarea contactelor
  rămâne verificarea numerelor de siguranță.
- **Monitorizarea e O(n)** în lățime de bandă la primul self-check al unui dispozitiv (~150 B/intrare);
  potrivită pentru servere de mărimea unei comunități, nu pentru milioane de conturi (acolo: prefix tree
  + VRF ca Signal). Jurnalul expune lista de ID-uri de cont și data creării (nu numerele de telefon).
- Dispozitivele (nu doar identitatea contului) nu sunt în jurnal: un dispozitiv injectat e deja refuzat
  de pinning, fiindcă trebuie semnat de identitatea contului.
- Clienții cu un server fără `kt_public_key` funcționează ca înainte (TOFU).
