# Grupuri private cu zkgroup — design

Scop: serverul nu mai știe **cine e în ce grup** și nu poate **adăuga membri** pe care niciun membru nu i-a
adăugat. Folosim `zkgroup` din libsignal (același tag `v0.103.1` ca restul), sistemul de grupuri private al
Signal ("The Signal Private Group System", Chase–Perrin–Zaverucha 2019). Se implementează doar în serverul Rust
(`server-rs`); serverul Go rămâne pe grupurile v1.

Stratul existent de membri confirmați end-to-end (`chat-core/src/engine/attest.rs`) rămâne activ peste
zkgroup: cele două se completează.

## Concepte

| Element | Cine îl știe | Rol |
|---|---|---|
| `ServerSecretParams` | doar serverul | derivat determinist din `CHATAPP_SERVER_SECRET` (ca cheile sealed sender) |
| `ServerPublicParams` | toți | în `/v1/info` (`zkgroup_public_params`); clientul îl fixează la primul contact |
| `GroupMasterKey` (32 B) | doar membrii | aleator, creat de fondator, distribuit E2E (înlocuiește actuala cheie de metadate) |
| `GroupSecretParams` / `GroupPublicParams` | membrii / serverul | derivate din master key; serverul identifică grupul după `GroupPublicParams` |
| `UuidCiphertext` | serverul | ID-ul de cont al unui membru, criptat determinist cu cheia grupului |
| `AuthCredential` (zkc, fără PNI) | clientul | emis zilnic de server; dovedește „sunt contul X” fără să spună serverului care |
| `AuthCredentialPresentation` | serverul o verifică | „sunt membrul al cărui ciphertext e C în grupul G” |
| `GroupSendEndorsement` | membrii | permite trimiterea în grup fără ca serverul să afle grupul sau expeditorul |

## Ce stochează serverul (grupuri v2)

- `groups_v2(id = group identifier, public_params, revision, state)` — `state` e criptat cu cheia grupului
  (nume, descriere, avatar, reguli); serverul vede doar revizia.
- `group_members_v2(group_id, uid_ciphertext, role, joined_revision)` — **niciun ID de cont în clar**.
- Invitații și cereri de intrare: tot ca ciphertext-uri.

Autorizarea fiecărei operații: clientul trimite o prezentare de credențial; serverul o verifică cu
`verify_auth_credential_presentation`, află `uid_ciphertext`-ul apelantului și verifică rolul acestuia în grup —
fără să afle cine este.

## Etape

1. **Fundația criptografică** — gata
   - server: parametri zkgroup derivați din secret, publicați în `/v1/info`; endpoint
     `GET /v1/certificate/auth/group?redemption_start_seconds=…&redemption_end_seconds=…` care emite
     credențiale zilnice (max. 7 zile).
   - client: fixează parametrii publici ai serverului, descarcă și **verifică** credențialele, le păstrează local,
     produce prezentări pentru un grup.
   - teste: ciclul complet emitere → primire → prezentare → verificare, și test end-to-end client–server.
2. **Stocarea grupurilor v2 pe server** — gata: tabele noi, API v2 (creare, citire, modificări ca acțiuni criptate,
   revizii), autorizare prin prezentări.
3. **Grupuri v2 în client** — gata: master key, criptarea/decriptarea membrilor, sincronizare din starea serverului,
   operațiile de admin; aplicațiile (desktop, mobil) neschimbate la nivel de interfață.
4. **Trimiterea în grup fără ca serverul să afle grupul**: group send endorsements + sealed sender; clientul
   alege destinatarii din lista decriptată și trimite un mesaj multi-destinatar cu token de endorsement.
5. **Invitații prin link v2** (parolă de link, cereri ca ciphertext-uri).
6. **Migrare și retragerea grupurilor v1** (API vechi eliminat din serverul Rust).

## Stare: etapele 2 și 3 (implementat)

**Server** (`server-rs/src/api/groups_v2.rs`, migrarea `0013_groups_v2.sql`, API: `docs/API.md`,
secțiunea „Private groups v2”):
- Tabele `groups_v2` (id = `GroupIdentifier`, parametrii publici, revizia, blob-urile criptate
  `attributes` — nume/descriere/avatar — și `settings` — timer, reguli —, nivelurile de acces în
  clar: cine editează atributele, cine adaugă membri), `group_members_v2` (`uid_ciphertext`, rol,
  `joined_revision`), `group_pending_v2` (invitați: ciphertext, rol, cine i-a invitat),
  `group_changes_v2` (jurnalul de modificări: JSON cu ciphertext-uri și blob-uri, semnat de server
  cu `ServerSecretParams::sign`; clientul verifică semnătura cu parametrii fixați).
- Cererile v2 sunt **anonime**: fără token de cont, doar antetul
  `Group-Auth: <GroupPublicParams>:<prezentare>`. Serverul verifică prezentarea, află
  ciphertext-ul apelantului și verifică rolul pe ciphertext-uri. Autorizarea e o funcție pură
  (`apply`) testată unitar: ne-membru respins, membru fără rol nu poate adăuga/elimina/promova,
  parametri greșiți respinși, revizie veche → 409, ultimul admin plecat → promovează cel mai vechi
  membru, ultimul membru plecat → grupul e șters.
- `/v1/info`: `groups_v2: true` (API-ul există) și `groups_v2_default`
  (`CHATAPP_GROUPS_V2_DEFAULT`, implicit **false**): grupurile noi devin v2 implicit abia când
  operatorul o cere — invitațiile prin link (etapa 5) există doar pentru v1, iar până atunci un
  grup nou v2 ar pierde funcția de link.

**Client** (`crates/chat-core/src/engine/groups_v2.rs`, cârlige mici în `groups.rs`, `send.rs`,
`invites.rs`, `zk.rs`):
- Cheia master (32 B aleatori) circulă exact ca vechea cheie de grup: în mesajele de control
  pereche (și în seif), deci stratul de membri atestați (`attest.rs`) funcționează neschimbat
  deasupra. ID-ul local al grupului = UUID derivat HKDF din cheia master; serverul nu îl poate lega
  de `GroupIdentifier`. Un grup e v2 dacă cheia lui derivă ID-ul; o cheie care nu-l derivă e
  refuzată (un admin nu poate „muta” grupul).
- Sincronizarea citește starea cu o prezentare, decriptează membrii și blob-urile și o scrie în
  același rând local ca la v1 (`apply_group_state`), deci desktop/mobil folosesc aceleași RPC-uri
  și modele. 403/404 → am ieșit din grup. Dacă suntem invitați (pending), clientul acceptă.
- Operațiile de admin (adăugare, eliminare, rol, nume/descriere/avatar, reguli, ieșire) merg prin
  `PATCH /v2/groups`; modificările sunt anunțate membrilor tot prin mesajele de control E2E (care
  declanșează sincronizarea). Membrul eliminat primește și el anunțul; serverul îi refuză apoi
  prezentările.
- Un membru apare ca membru doar dacă e și în starea v2 a serverului, și atestat E2E; cheile de
  expeditor și mesajele de grup merg doar la intersecție (serverul nu poate adăuga pe nimeni).
- Crearea: v2 când serverul are capabilitatea și `groups_v2_default`; `create_group_v2` o forțează
  (testele). Grupurile v1 existente rămân v1.
- Teste: unitare (criptarea/decriptarea intrărilor de membri, blob-uri tipizate și cu padding,
  ID local nelegabil, decriptarea stării), server (`groups_v2::tests`), e2e
  `crates/chat-core/tests/e2e_groups_v2.rs` (ciclul complet + verificarea că tabelele v2 nu conțin
  ID-uri de cont sau numele grupului; autorizarea serverului cu credențiale reale).

**Limitări cunoscute / rămas de făcut**
- Trimiterea în grup folosește tot sender keys, dar pe ruta `PUT /v2/groups/messages`: serverul
  vede expeditorul și lista de destinatari (nu și grupul). Etapa 4 (endorsements + sealed sender).
- `only_admins_send`, `hide_members`, `member_names` sunt în blob-ul criptat de setări și sunt
  aplicate de clienți (`only_admins_send` doar de clientul expeditorului), nu de server (serverul
  nu știe cine trimite în grup); verificarea la destinatar rămâne de făcut.
- La eliminarea unui membru cheia master nu se schimbă (ea identifică grupul, ca la Signal): fostul
  membru nu mai poate citi starea fiindcă serverul îi refuză prezentările.
- Serverul ar putea servi o stare mai veche (rollback); jurnalul semnat permite detectarea, dar
  clientul nu compară încă revizia cu cea din mesajele de control.
- Invitații prin link pentru v2 (etapa 5): `set_group_invite` refuză grupurile v2.
- Migrarea v1 → v2 și retragerea API-ului v1 (etapa 6): neîncepute.

## Ce rămâne vizibil pentru server

- Destinatarii fiecărui mesaj (ca la orice mesagerie cu server central); grupul și expeditorul nu.
- Numărul de membri și momentul modificărilor unui grup (nu și cine).
- Adresele IP.

## Riscuri și decizii

- Parametrii serverului derivă din secretul serverului: schimbarea secretului invalidează credențialele
  (clientele le reiau automat) și **grupurile v2 rămân valabile** (identificate după parametrii grupului, nu ai
  serverului). Clientul refuză parametri publici schimbați pentru același server fără confirmarea
  utilizatorului.
- Serverul Go nu primește grupuri v2.
- Sesiunea din cloud dezvoltă în paralel funcții de grup v1; etapele 2–6 trebuie coordonate cu ea.
