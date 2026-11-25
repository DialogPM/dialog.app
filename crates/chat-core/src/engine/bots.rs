//! Bots (docs/API.md section 22).
//!
//! A bot is a program on someone's server that talks through the Dialog server's Bot API. In
//! the app it is a direct conversation whose peer is the bot's id, with three differences:
//!
//! * Nothing is end-to-end encrypted: a bot has no keys. What it writes arrives in an
//!   envelope of type 9, as text, and what we write goes to the server as text
//!   (`POST /v1/bots/{id}/messages`). The UI says so in every bot conversation.
//! * The bot can write to us only after we start it and until we stop it. The server enforces
//!   that; this device only shows the state.
//! * The bot never learns who we are: no account id, number, name, profile or presence.
//!   Everything else a conversation sends its peer (receipts, typing, profile key, calls) is
//!   dropped in `send_to_bot`.
//!
//! Our own bots (the ones this account created) are managed with the `bots_*` functions; the
//! token of a bot is shown once, when it is created or reset, and never stored here.

use std::rc::Rc;

use rusqlite::{params, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};

use super::Inner;
use crate::api::ApiErrorOrNet;
use crate::content::{AttachmentPointer, Content, DataMessage};
use crate::error::{CoreError, ErrorCode, Result};
use crate::model::{BotButton, BotCommand, BotCreated, BotInfo, BotSubscriber, Conversation, OwnedBot};
use crate::util::{is_id, new_id, now_ms};

/// Envelope type of a bot's message (plain JSON content; only the server creates it).
pub(super) const BOT_ENVELOPE: u8 = 9;
/// Longest text either way (the server's limit).
const MAX_BOT_TEXT: usize = 4096;

/// A bot as the server describes it to someone talking to it.
#[derive(Deserialize)]
struct BotDto {
    id: String,
    username: String,
    name: String,
    #[serde(default)]
    about: String,
    #[serde(default)]
    commands: Vec<BotCommand>,
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    mine: bool,
}

#[derive(Deserialize)]
struct BotChatDto {
    bot: BotDto,
    state: String,
}

#[derive(Deserialize)]
struct BotChatsDto {
    #[serde(default)]
    chats: Vec<BotChatDto>,
}

#[derive(Deserialize)]
struct OwnedBotsDto {
    #[serde(default)]
    bots: Vec<OwnedBot>,
}

/// Content of a type 9 envelope.
#[derive(Deserialize)]
struct BotMessage {
    id: String,
    #[serde(default)]
    text: String,
    #[serde(default)]
    sent_at: i64,
    #[serde(default)]
    photo: Option<BotPhoto>,
    #[serde(default)]
    buttons: Vec<Vec<BotButton>>,
}

/// A picture a bot sent: a server attachment, stored as the bot uploaded it (not encrypted).
#[derive(Deserialize)]
struct BotPhoto {
    id: String,
    #[serde(default)]
    mime: String,
    #[serde(default)]
    size: i64,
}

/// The server's limits, enforced here too (the server is not trusted to keep its own).
const MAX_BUTTON_ROWS: usize = 8;
const MAX_BUTTONS_PER_ROW: usize = 4;
const MAX_BUTTON_TEXT: usize = 40;
const MAX_BUTTON_DATA: usize = 64;
const MAX_BUTTON_URL: usize = 512;
const MAX_BOT_PHOTO: i64 = 10 << 20;

/// The buttons worth showing: text and exactly one of data or an https link.
fn clean_buttons(rows: Vec<Vec<BotButton>>) -> Vec<Vec<BotButton>> {
    rows.into_iter()
        .take(MAX_BUTTON_ROWS)
        .map(|row| {
            row.into_iter()
                .take(MAX_BUTTONS_PER_ROW)
                .filter_map(|b| {
                    let text: String = crate::sanitize::clean_display_text(&b.text, MAX_BUTTON_TEXT).trim().to_string();
                    let data = b.data.filter(|d| !d.is_empty() && d.len() <= MAX_BUTTON_DATA && !d.chars().any(char::is_control));
                    let url = b.url.filter(|u| {
                        u.len() <= MAX_BUTTON_URL && u.starts_with("https://") && !u.chars().any(|c| c.is_control() || c.is_whitespace())
                    });
                    (!text.is_empty() && data.is_some() != url.is_some()).then_some(BotButton { text, data, url })
                })
                .collect::<Vec<_>>()
        })
        .filter(|row| !row.is_empty())
        .collect()
}

fn state_of(s: Option<&str>) -> &'static str {
    match s {
        Some("active") => "active",
        Some("stopped") => "stopped",
        _ => "none",
    }
}

/// The `reason` of a refusal by the bots endpoints, mapped to what the user can do about it.
fn bot_error(e: ApiErrorOrNet) -> CoreError {
    match &e {
        ApiErrorOrNet::Api(a) => match a.body.get("reason").and_then(Value::as_str) {
            Some("bot_not_started") => CoreError::new(ErrorCode::Forbidden, "start the bot first"),
            Some("bot_disabled") => CoreError::new(ErrorCode::Forbidden, "this bot is disabled"),
            Some("username_taken") => CoreError::new(ErrorCode::Conflict, "this username is taken"),
            Some("too_many_bots") => CoreError::new(ErrorCode::Conflict, "you have reached the limit of bots for one account"),
            _ if a.status == 404 => CoreError::not_found("this bot no longer exists"),
            _ => e.into(),
        },
        _ => e.into(),
    }
}

impl Inner {
    /// Whether `account_id` is a bot this device knows.
    pub fn is_bot(&self, account_id: &str) -> bool {
        self.db
            .borrow()
            .conn
            .query_row("SELECT 1 FROM bots WHERE account_id = ?1", [account_id], |_| Ok(()))
            .optional()
            .ok()
            .flatten()
            .is_some()
    }

    /// The bot behind a conversation peer, as stored here.
    pub fn bot_info(&self, account_id: &str) -> Result<Option<BotInfo>> {
        let row: Option<(String, String, String, String, String, bool, bool)> = self
            .db
            .borrow()
            .conn
            .query_row(
                "SELECT username, name, about, commands_json, state, mine, deleted FROM bots WHERE account_id = ?1",
                [account_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?)),
            )
            .optional()?;
        Ok(row.map(|(username, name, about, commands, state, mine, deleted)| BotInfo {
            id: account_id.to_string(),
            username,
            name,
            about,
            commands: serde_json::from_str(&commands).unwrap_or_default(),
            state: state_of(Some(&state)).to_string(),
            mine,
            deleted,
        }))
    }

    /// Stores a bot's description; `state` None keeps the stored chat state.
    fn store_bot(&self, b: &BotDto, state: Option<&str>) -> Result<()> {
        if !is_id(&b.id) {
            return Err(CoreError::internal("malformed bot id"));
        }
        let commands = serde_json::to_string(&b.commands)?;
        let db = self.db.borrow();
        db.conn.execute(
            "INSERT INTO bots (account_id, username, name, about, commands_json, state, mine, deleted, fetched_at)
             VALUES (?1, ?2, ?3, ?4, ?5, COALESCE(?6, 'none'), ?7, 0, ?8)
             ON CONFLICT(account_id) DO UPDATE SET username = excluded.username, name = excluded.name,
                about = excluded.about, commands_json = excluded.commands_json,
                state = COALESCE(?6, state), mine = excluded.mine, deleted = 0, fetched_at = excluded.fetched_at",
            params![b.id, b.username, b.name, b.about, commands, state.map(|s| state_of(Some(s))), b.mine, now_ms()],
        )?;
        // The contact row gives the conversation its title; a bot has no number and no keys.
        db.conn.execute(
            "INSERT INTO contacts (account_id, number, profile_name, about, updated_at) VALUES (?1, '', ?2, ?3, 0)
             ON CONFLICT(account_id) DO UPDATE SET profile_name = excluded.profile_name, about = excluded.about",
            params![b.id, b.name, b.about],
        )?;
        Ok(())
    }

    /// Asks the server about a bot and stores what it says. Ok(false): no such bot.
    pub(super) async fn fetch_bot(&self, bot_id: &str) -> Result<bool> {
        if !is_id(bot_id) {
            return Ok(false);
        }
        let api = self.session()?.api.clone();
        match api.send_json::<(), BotDto>(reqwest::Method::GET, &format!("/v1/bots/{bot_id}"), None).await {
            Ok(b) if b.id == bot_id => {
                self.store_bot(&b, b.state.as_deref())?;
                Ok(true)
            }
            Ok(_) => Ok(false),
            Err(ApiErrorOrNet::Api(a)) if a.status == 404 => {
                self.db.borrow().conn.execute("UPDATE bots SET deleted = 1, state = 'none' WHERE account_id = ?1", [bot_id])?;
                Ok(false)
            }
            Err(e) => Err(e.into()),
        }
    }

    /// The conversation with a bot this device knows, created if needed (always accepted:
    /// a bot conversation exists only because we opened or started it).
    fn ensure_bot_conversation(&self, bot_id: &str) -> Result<String> {
        if let Some(id) = self.conversation_id_for_peer(bot_id)? {
            return Ok(id);
        }
        let (id, now) = (new_id(), now_ms());
        self.db.borrow().conn.execute(
            "INSERT OR IGNORE INTO conversations (id, kind, peer_account_id, created_at, updated_at, accepted) VALUES (?1, 'direct', ?2, ?3, ?3, 1)",
            params![id, bot_id, now],
        )?;
        self.conversation_id_for_peer(bot_id)?.ok_or_else(|| CoreError::internal("bot conversation not created"))
    }

    fn emit_bot_conversation(&self, bot_id: &str) {
        if let Ok(Some(id)) = self.conversation_id_for_peer(bot_id) {
            self.emit_conversation(&id);
        }
    }

    // --- talking to a bot -----------------------------------------------------------------

    /// Looks a bot up by its username (`weatherbot`, `@weatherbot`). Nothing is stored and no
    /// conversation is created: the user decides after seeing what the bot is.
    pub async fn bot_find(&self, username: &str) -> Result<BotInfo> {
        let name = username.trim().trim_start_matches('@').to_ascii_lowercase();
        if name.is_empty() || !name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'.') {
            return Err(CoreError::not_found("no bot with this username"));
        }
        let api = self.session()?.api.clone();
        let b: BotDto = api
            .send_json::<(), _>(reqwest::Method::GET, &format!("/v1/bots/username/{name}"), None)
            .await
            .map_err(|e| match e {
                ApiErrorOrNet::Api(a) if a.status == 404 => CoreError::not_found("no bot with this username"),
                e => e.into(),
            })?;
        Ok(BotInfo {
            state: state_of(b.state.as_deref()).to_string(),
            id: b.id,
            username: b.username,
            name: b.name,
            about: b.about,
            commands: b.commands,
            mine: b.mine,
            deleted: false,
        })
    }

    /// Opens the conversation with a bot (refreshing its description and commands). The bot
    /// still cannot write to us: that takes `bot_start`.
    pub async fn bot_open(&self, bot_id: &str) -> Result<Conversation> {
        match self.fetch_bot(bot_id).await {
            Ok(true) => {}
            Ok(false) if self.is_bot(bot_id) => {} // deleted: the conversation stays readable
            Ok(false) => return Err(CoreError::not_found("this bot no longer exists")),
            Err(e) if e.is_network() && self.is_bot(bot_id) => {}
            Err(e) => return Err(e),
        }
        let conv = self.ensure_bot_conversation(bot_id)?;
        self.emit_conversation(&conv);
        self.get_conversation(&conv)
    }

    /// Starts (or restarts) the bot: from now on it may write to us. The server hands the bot
    /// the message "/start"; the conversation notes the start.
    pub async fn bot_start(self: &Rc<Self>, bot_id: &str) -> Result<Conversation> {
        if !self.is_bot(bot_id) && !self.fetch_bot(bot_id).await? {
            return Err(CoreError::not_found("this bot no longer exists"));
        }
        let api = self.session()?.api.clone();
        let _: Value = api
            .send_json(reqwest::Method::POST, &format!("/v1/bots/{bot_id}/start"), Some(&json!({})))
            .await
            .map_err(bot_error)?;
        let restart = self.bot_info(bot_id)?.is_some_and(|b| b.state == "stopped");
        self.set_bot_state(bot_id, "active")?;
        let conv = self.ensure_bot_conversation(bot_id)?;
        self.insert_system_message(&conv, if restart { "bot_restarted" } else { "bot_started" }, None, vec![], None, None, now_ms())?;
        self.emit_conversation(&conv);
        self.get_conversation(&conv)
    }

    /// Stops the bot: the server refuses whatever it sends us until we start it again.
    pub async fn bot_stop(&self, bot_id: &str) -> Result<Conversation> {
        let api = self.session()?.api.clone();
        let stopped: std::result::Result<Value, _> =
            api.send_json(reqwest::Method::POST, &format!("/v1/bots/{bot_id}/stop"), Some(&json!({}))).await;
        match stopped {
            Ok(_) => {}
            // Never started, or the bot is gone: nothing can reach us either way.
            Err(ApiErrorOrNet::Api(a)) if a.status == 404 => {}
            Err(e) => return Err(e.into()),
        }
        let was_active = self.bot_info(bot_id)?.is_some_and(|b| b.state == "active");
        self.set_bot_state(bot_id, "stopped")?;
        let conv = self.ensure_bot_conversation(bot_id)?;
        if was_active {
            self.insert_system_message(&conv, "bot_stopped", None, vec![], None, None, now_ms())?;
        }
        self.emit_conversation(&conv);
        self.get_conversation(&conv)
    }

    fn set_bot_state(&self, bot_id: &str, state: &str) -> Result<()> {
        self.db.borrow().conn.execute("UPDATE bots SET state = ?2 WHERE account_id = ?1", params![bot_id, state_of(Some(state))])?;
        Ok(())
    }

    /// What may be sent to a bot: text, in a started chat.
    pub(super) fn check_can_send_to_bot(&self, bot_id: &str, text_only: bool) -> Result<()> {
        let Some(bot) = self.bot_info(bot_id)? else { return Ok(()) };
        if bot.deleted {
            return Err(CoreError::new(ErrorCode::Forbidden, "this bot no longer exists"));
        }
        if bot.state != "active" {
            return Err(CoreError::new(ErrorCode::Forbidden, "start the bot first"));
        }
        if !text_only {
            return Err(CoreError::invalid("bots receive text messages only"));
        }
        Ok(())
    }

    /// `send_to_account` for a bot peer: a plain text message goes to the server as text;
    /// everything else a conversation would send its peer is not for bots and is dropped.
    pub(super) async fn send_to_bot(&self, bot_id: &str, content: &Content) -> Result<()> {
        let Content::Data(d) = content else { return Ok(()) };
        let plain = d.reaction.is_none() && d.edit.is_none() && d.delete.is_none() && !d.timer_update;
        let Some(text) = d.body.as_deref().filter(|t| plain && !t.trim().is_empty()) else { return Ok(()) };
        if text.chars().count() > MAX_BOT_TEXT {
            return Err(CoreError::invalid("message is too long"));
        }
        let api = self.session()?.api.clone();
        let sent: std::result::Result<Value, _> = api
            .send_json(reqwest::Method::POST, &format!("/v1/bots/{bot_id}/messages"), Some(&json!({"id": d.id, "text": text})))
            .await;
        match sent {
            Ok(_) => Ok(()),
            Err(ApiErrorOrNet::Api(a)) if a.status == 404 => {
                self.db.borrow().conn.execute("UPDATE bots SET deleted = 1, state = 'none' WHERE account_id = ?1", [bot_id])?;
                self.emit_bot_conversation(bot_id);
                Err(CoreError::new(ErrorCode::AccountGone, "this bot no longer exists"))
            }
            Err(e) => {
                let e = bot_error(e);
                if e.code == ErrorCode::Forbidden {
                    // Stopped on another device meanwhile.
                    self.set_bot_state(bot_id, "stopped")?;
                    self.emit_bot_conversation(bot_id);
                }
                Err(e)
            }
        }
    }

    /// A bot's message (envelope type 9, from the server, not encrypted).
    pub(super) async fn handle_bot_envelope(self: &Rc<Self>, bot_id: &str, content: &[u8]) -> Result<()> {
        let Ok(m) = serde_json::from_slice::<BotMessage>(content) else {
            log::warn!("dropping a malformed bot message");
            return Ok(());
        };
        let photo = m.photo.filter(|p| {
            p.size > 0 && p.size <= MAX_BOT_PHOTO && !p.id.is_empty() && p.id.len() <= 64 && p.id.bytes().all(|b| b.is_ascii_alphanumeric())
        });
        if !is_id(&m.id) || (m.text.trim().is_empty() && photo.is_none()) || m.text.chars().count() > MAX_BOT_TEXT {
            log::warn!("dropping a malformed bot message");
            return Ok(());
        }
        // Only a bot the server vouches for: an account cannot pass for one.
        if !self.is_bot(bot_id) {
            match self.fetch_bot(bot_id).await {
                Ok(true) => {}
                Ok(false) => {
                    log::warn!("dropping a bot message from an unknown bot");
                    return Ok(());
                }
                Err(e) => return Err(e),
            }
        }
        // The server delivers only into a started chat.
        self.db.borrow().conn.execute("UPDATE bots SET state = 'active' WHERE account_id = ?1 AND state = 'none'", [bot_id])?;
        let conv = self.ensure_bot_conversation(bot_id)?;
        let buttons = clean_buttons(m.buttons);
        if !buttons.is_empty() {
            // Before the message itself, so that it is shown with its buttons from the start.
            self.db.borrow().conn.execute(
                "INSERT OR IGNORE INTO bot_buttons (message_id, json) VALUES (?1, ?2)",
                params![m.id, serde_json::to_string(&buttons)?],
            )?;
        }
        // An empty key marks the picture as not encrypted; `download_attachment` accepts
        // that from a bot only.
        let attachments = photo
            .map(|p| AttachmentPointer {
                cdn_id: p.id,
                key: String::new(),
                digest: String::new(),
                size: p.size,
                mime: p.mime,
                file_name: "photo".into(),
                kind: "image".into(),
                width: None,
                height: None,
                duration_ms: None,
                waveform: None,
                thumbnail: None,
            })
            .into_iter()
            .collect();
        let body = Some(m.text).filter(|t| !t.trim().is_empty());
        let data = DataMessage { id: m.id, body, attachments, sent_at: m.sent_at, ..Default::default() };
        self.apply_data(&conv, bot_id, 0, data, false)
    }

    /// The buttons under a bot's message (none for any other message).
    pub(super) fn bot_buttons(&self, message_id: &str) -> Vec<Vec<BotButton>> {
        let json: Option<String> = self
            .db
            .borrow()
            .conn
            .query_row("SELECT json FROM bot_buttons WHERE message_id = ?1", [message_id], |r| r.get(0))
            .optional()
            .ok()
            .flatten();
        json.and_then(|j| serde_json::from_str(&j).ok()).unwrap_or_default()
    }

    /// The user pressed a button of a bot's message: the bot is told the button's data.
    pub async fn bot_press(&self, message_id: &str, data: &str) -> Result<()> {
        let row = self.message_row(message_id)?.ok_or_else(|| CoreError::not_found("message not found"))?;
        let known = self.bot_buttons(message_id).iter().flatten().any(|b| b.data.as_deref() == Some(data));
        if row.outgoing || !known || !self.is_bot(&row.sender_account_id) {
            return Err(CoreError::invalid("this message has no such button"));
        }
        let bot_id = row.sender_account_id;
        self.check_can_send_to_bot(&bot_id, true)?;
        let api = self.session()?.api.clone();
        let _: Value = api
            .send_json(reqwest::Method::POST, &format!("/v1/bots/{bot_id}/callback"), Some(&json!({"message_id": message_id, "data": data})))
            .await
            .map_err(bot_error)?;
        Ok(())
    }

    /// The server's word on every bot this account ever started: restores bot conversations
    /// on a new device and picks up starts and stops made on our other devices.
    pub(super) async fn sync_bot_chats(&self) -> Result<()> {
        let api = self.session()?.api.clone();
        let dto: BotChatsDto = api.get("/v1/bot-chats").await?;
        for c in &dto.chats {
            let before = self.bot_info(&c.bot.id)?;
            self.store_bot(&c.bot, Some(&c.state))?;
            let changed = before.is_none_or(|b| b.state != state_of(Some(&c.state)) || b.name != c.bot.name || b.commands != c.bot.commands);
            if changed {
                self.emit_bot_conversation(&c.bot.id);
            }
        }
        Ok(())
    }

    pub(super) fn spawn_bot_chats_sync(self: &Rc<Self>) {
        let me = self.clone();
        tokio::task::spawn_local(async move {
            if let Err(e) = me.sync_bot_chats().await {
                log::debug!("bot chats: {e}");
            }
        });
    }

    /// Server event: the chat with a bot was started or stopped on another of our devices.
    pub(super) fn on_bot_chat_event(self: &Rc<Self>, bot_id: String, state: String) {
        if self.is_bot(&bot_id) {
            let _ = self.set_bot_state(&bot_id, &state);
            self.emit_bot_conversation(&bot_id);
        } else {
            self.spawn_bot_chats_sync();
        }
    }

    /// Server event: a bot we talked to was deleted by its owner.
    pub(super) fn on_bot_deleted_event(&self, bot_id: &str) {
        let _ = self.db.borrow().conn.execute("UPDATE bots SET deleted = 1, state = 'none' WHERE account_id = ?1", [bot_id]);
        self.emit_bot_conversation(bot_id);
    }

    // --- our own bots ---------------------------------------------------------------------

    pub async fn bots_list(&self) -> Result<Vec<OwnedBot>> {
        let api = self.session()?.api.clone();
        let mut dto: OwnedBotsDto = api.get("/v1/bots").await?;
        for b in &mut dto.bots {
            b.token = self.bot_token(&b.id);
        }
        Ok(dto.bots)
    }

    /// Creates a bot. The token in the result is shown to the user once; it is not kept.
    pub async fn bot_create(&self, username: &str, name: &str, about: &str) -> Result<BotCreated> {
        let api = self.session()?.api.clone();
        let mut created: BotCreated = api
            .send_json(reqwest::Method::POST, "/v1/bots", Some(&json!({"username": username, "name": name, "about": about})))
            .await
            .map_err(|e| match e {
                ApiErrorOrNet::Api(a) if a.status == 400 => CoreError::invalid(a.message),
                e => bot_error(e),
            })?;
        self.keep_bot_token(&created.bot.id, &created.token)?;
        created.bot.token = Some(created.token.clone());
        Ok(created)
    }

    /// The token of one of our bots, if this device created or reset it. It lives in this
    /// device's encrypted database only: the server has just its hash.
    fn bot_token(&self, id: &str) -> Option<String> {
        let raw = self.db.borrow().kv_get(&format!("bot_token:{id}")).ok().flatten()?;
        String::from_utf8(raw).ok()
    }

    fn keep_bot_token(&self, id: &str, token: &str) -> Result<()> {
        self.db.borrow().kv_set(&format!("bot_token:{id}"), token.as_bytes())
    }

    /// The chats one of our bots may write to (chat ids; nobody's identity).
    pub async fn bot_subscribers(&self, id: &str) -> Result<Vec<BotSubscriber>> {
        #[derive(Deserialize)]
        struct Dto {
            #[serde(default)]
            subscribers: Vec<BotSubscriber>,
        }
        let api = self.session()?.api.clone();
        let dto: Dto = api.send_json::<(), _>(reqwest::Method::GET, &format!("/v1/bots/{id}/subscribers"), None).await.map_err(bot_error)?;
        Ok(dto.subscribers)
    }

    /// Sends `text` as one of our bots to one of its chats (the settings' test message).
    pub async fn bot_send_test(&self, id: &str, chat_id: &str, text: &str) -> Result<()> {
        let api = self.session()?.api.clone();
        let _: Value = api
            .send_json(reqwest::Method::POST, &format!("/v1/bots/{id}/send"), Some(&json!({"chat_id": chat_id, "text": text})))
            .await
            .map_err(|e| match e {
                ApiErrorOrNet::Api(a) if a.body.get("reason").and_then(Value::as_str) == Some("bot_stopped") => {
                    CoreError::new(ErrorCode::Forbidden, "this person stopped the bot")
                }
                ApiErrorOrNet::Api(a) if a.status == 400 => CoreError::invalid(a.message),
                e => bot_error(e),
            })?;
        Ok(())
    }

    pub async fn bot_update(&self, id: &str, name: Option<String>, about: Option<String>, commands: Option<Vec<BotCommand>>) -> Result<OwnedBot> {
        let mut body = serde_json::Map::new();
        if let Some(v) = name {
            body.insert("name".into(), v.into());
        }
        if let Some(v) = about {
            body.insert("about".into(), v.into());
        }
        if let Some(v) = commands {
            body.insert("commands".into(), serde_json::to_value(v)?);
        }
        let api = self.session()?.api.clone();
        let bot: OwnedBot = api
            .send_json(reqwest::Method::PATCH, &format!("/v1/bots/{id}"), Some(&Value::Object(body)))
            .await
            .map_err(|e| match e {
                ApiErrorOrNet::Api(a) if a.status == 400 => CoreError::invalid(a.message),
                e => bot_error(e),
            })?;
        // Our own conversation with it shows the new name and commands right away.
        if self.is_bot(id) {
            let _ = self.fetch_bot(id).await;
            self.emit_bot_conversation(id);
        }
        Ok(bot)
    }

    /// A new token for the bot; the old one stops working at once.
    pub async fn bot_reset_token(&self, id: &str) -> Result<String> {
        #[derive(Deserialize)]
        struct Token {
            token: String,
        }
        let api = self.session()?.api.clone();
        let t: Token = api.send_json(reqwest::Method::POST, &format!("/v1/bots/{id}/token"), Some(&json!({}))).await.map_err(bot_error)?;
        self.keep_bot_token(id, &t.token)?;
        Ok(t.token)
    }

    /// Deletes the bot for good: its token stops working and nobody can write to it.
    pub async fn bot_delete(&self, id: &str) -> Result<()> {
        let api = self.session()?.api.clone();
        api.delete::<()>(&format!("/v1/bots/{id}"), None).await?;
        self.db.borrow().kv_delete(&format!("bot_token:{id}"))?;
        self.on_bot_deleted_event(id);
        Ok(())
    }
}
