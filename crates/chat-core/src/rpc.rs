//! Command dispatcher shared by every host (Tauri desktop app, Flutter mobile app).
//!
//! Hosts forward `(command name, JSON request)` pairs; the reply is JSON. The command names
//! and request/response shapes are the contract documented in `app/src/lib/backend.ts`.

use std::rc::Rc;

use serde::Deserialize;
use serde_json::Value;

use crate::engine::{ConnState, Core, DndState, Inner};
use crate::engine::invites::{InvitePreview, JoinRequest, JoinResult, SetInviteRequest};
use crate::engine::security::{LoginRequestInfo, NewDeviceAlert};
use crate::engine::keytrans::{KtContactStatus, KtOverview};
use crate::engine::username_link::UsernameLink;
use crate::error::{CoreError, Result};
use crate::model::*;

#[derive(Deserialize)]
pub struct UrlReq {
    url: String,
}
#[derive(Deserialize)]
pub struct RegisterReq {
    server_url: String,
    password: String,
    device_name: String,
}
#[derive(Deserialize)]
pub struct LoginReq {
    server_url: String,
    number: String,
    password: String,
    device_name: String,
    /// Two-step verification code, when the account has it enabled.
    #[serde(default)]
    totp: Option<String>,
}
#[derive(Deserialize)]
pub struct TotpCodeReq {
    code: String,
}
#[derive(Deserialize)]
pub struct RecoverReq {
    server_url: String,
    number: String,
    mnemonic: Vec<String>,
    new_password: String,
    device_name: String,
}
#[derive(Deserialize)]
pub struct ChangePasswordReq {
    old_password: String,
    new_password: String,
}
#[derive(Deserialize)]
pub struct ExportMediaReq {
    path: String,
    dest: String,
}
#[derive(Deserialize)]
pub struct PasswordReq {
    password: String,
}
#[derive(Deserialize)]
pub struct ReminderReq {
    name: String,
}
#[derive(Deserialize)]
pub struct EnabledReq {
    enabled: bool,
}
#[derive(Deserialize)]
pub struct MediaRetentionReq {
    /// 1, 7, 14 or 30.
    days: i64,
}
#[derive(Deserialize)]
pub struct DefaultDisappearingReq {
    /// One of the offered timers (engine/default_timer.rs), 0 = off.
    seconds: i64,
}
#[derive(Deserialize)]
pub struct DndReq {
    /// End (ms); 0 = off; `i64::MAX` (or any value past year 33658) = until turned off. A float
    /// is accepted too: JavaScript numbers cannot carry `i64::MAX` exactly.
    until: f64,
}
#[derive(Deserialize)]
pub struct PhoneReq {
    phone: String,
}
#[derive(Deserialize)]
pub struct VerifyPhoneReq {
    phone: String,
    code: String,
}
#[derive(Deserialize)]
pub struct DiscoverableReq {
    discoverable: bool,
}
#[derive(Deserialize)]
pub struct DeviceIdReq {
    device_id: u32,
}
#[derive(Deserialize)]
pub struct NameReq {
    name: String,
}
#[derive(Deserialize)]
pub struct AccountIdReq {
    account_id: String,
}
#[derive(Deserialize)]
pub struct AddByNumberReq {
    number: String,
    name: Option<String>,
}
#[derive(Deserialize)]
pub struct RenameContactReq {
    account_id: String,
    name: Option<String>,
}
#[derive(Deserialize)]
pub struct BlockReq {
    account_id: String,
    blocked: bool,
}
#[derive(Deserialize)]
pub struct VerifiedReq {
    account_id: String,
    verified: bool,
}
#[derive(Deserialize)]
pub struct PathReq {
    path: String,
}
#[derive(Deserialize)]
pub struct LinkReq {
    link: String,
}
#[derive(Deserialize)]
pub struct AccountIdsReq {
    account_ids: Vec<String>,
}
#[derive(Deserialize)]
pub struct ConvReq {
    conversation_id: String,
}
/// Destructive conversation actions: the account password confirms them.
#[derive(Deserialize)]
pub struct ConvPasswordReq {
    conversation_id: String,
    password: String,
}
#[derive(Deserialize)]
pub struct GroupIdReq {
    group_id: String,
}
#[derive(Deserialize)]
pub struct ConvAccountReq {
    conversation_id: String,
    account_id: String,
}
#[derive(Deserialize)]
pub struct PinReq {
    message_id: String,
    /// null unpins.
    #[serde(default)]
    seconds: Option<i64>,
}
#[derive(Deserialize)]
pub struct StarReq {
    message_id: String,
    starred: bool,
}
#[derive(Deserialize)]
pub struct DraftReq {
    conversation_id: String,
    draft: Option<String>,
}
#[derive(Deserialize)]
pub struct GetMessagesReq {
    conversation_id: String,
    before: Option<i64>,
    limit: Option<i64>,
}
#[derive(Deserialize)]
pub struct EditReq {
    message_id: String,
    text: String,
    #[serde(default)]
    mentions: Vec<Mention>,
}
#[derive(Deserialize)]
pub struct DeleteMsgReq {
    message_id: String,
    for_everyone: bool,
}
#[derive(Deserialize)]
pub struct ReactReq {
    message_id: String,
    emoji: Option<String>,
}
#[derive(Deserialize)]
pub struct ForwardReq {
    message_ids: Vec<String>,
    conversation_ids: Vec<String>,
}
#[derive(Deserialize)]
pub struct TypingReq {
    conversation_id: String,
    typing: bool,
}
#[derive(Deserialize)]
pub struct MessageIdReq {
    message_id: String,
}
#[derive(Deserialize)]
pub struct AttachmentReq {
    message_id: String,
    attachment_id: String,
}
#[derive(Deserialize)]
pub struct SaveAttachmentReq {
    message_id: String,
    attachment_id: String,
    dest_path: String,
}
#[derive(Deserialize)]
pub struct SearchReq {
    query: String,
    conversation_id: Option<String>,
    limit: Option<i64>,
}
#[derive(Deserialize)]
pub struct MediaReq {
    conversation_id: String,
    kind: String,
    limit: Option<i64>,
}
#[derive(Deserialize)]
pub struct MembersReq {
    conversation_id: String,
    account_ids: Vec<String>,
}
#[derive(Deserialize)]
pub struct MemberReq {
    conversation_id: String,
    account_id: String,
}
#[derive(Deserialize)]
pub struct AdminReq {
    conversation_id: String,
    account_id: String,
    admin: bool,
}
#[derive(Deserialize)]
pub struct LinkCreateReq {
    server_url: String,
    device_name: String,
}
#[derive(Deserialize)]
pub struct LinkClaimReq {
    link: String,
    device_name: String,
}
#[derive(Deserialize)]
pub struct LinkApproveReq {
    link: Option<String>,
}
#[derive(Deserialize)]
pub struct UsernameReq {
    username: Option<String>,
}
#[derive(Deserialize)]
pub struct QueryReq {
    query: String,
    #[serde(default)]
    name: Option<String>,
}
#[derive(Deserialize)]
pub struct AnswerLoginReq {
    id: String,
    approve: bool,
}
#[derive(Deserialize)]
pub struct ReportSpamReq {
    account_id: String,
    /// Also block the account (the usual "Report spam and block").
    #[serde(default = "default_true")]
    block: bool,
}
fn default_true() -> bool {
    true
}
#[derive(Deserialize)]
pub struct PresenceActiveReq {
    active: bool,
    #[serde(default)]
    since: Option<i64>,
}
#[derive(Deserialize)]
pub struct BackgroundReq {
    background: bool,
}
#[derive(Deserialize)]
pub struct PushTokenReq {
    /// "fcm", "apns_voip" or "" (clears push); also accepted as `kind`.
    #[serde(alias = "kind")]
    provider: String,
    #[serde(default)]
    token: String,
}

macro_rules! rpc_table {
    ($( $name:ident ( $($req:ident : $ty:ty)? ) -> $ret:ty => |$i:ident| $body:expr ; )*) => {
        /// Names of all commands.
        pub const COMMANDS: &[&str] = &[$(stringify!($name)),*];

        /// Runs a command on the engine thread and returns its JSON result.
        pub async fn dispatch(core: &Core, cmd: &str, req: Value) -> Result<Value> {
            match cmd {
                $(
                    stringify!($name) => {
                        $( let $req: $ty = serde_json::from_value(req.clone())
                            .map_err(|e| CoreError::invalid(format!("{}: {e}", stringify!($name))))?; )?
                        let _ = &req;
                        let out: $ret = core.run(move |$i: Rc<Inner>| async move { $body }).await?;
                        Ok(serde_json::to_value(out)?)
                    }
                )*
                _ => Err(CoreError::invalid(format!("unknown command {cmd}"))),
            }
        }
    };
}

rpc_table! {
    set_username(req: UsernameReq) -> Option<String> => |i| i.set_username(req.username).await;
    find_account(req: QueryReq) -> Option<FoundAccount> => |i| i.find_account(&req.query).await;
    add_contact_by_query(req: QueryReq) -> Contact => |i| i.add_contact_by_query(&req.query, req.name).await;
    list_blocked() -> Vec<Contact> => |i| i.list_blocked();
    get_state() -> AppState => |i| Ok(i.app_state());
    check_server(req: UrlReq) -> ServerInfo => |i| i.check_server(&req.url).await;
    register(req: RegisterReq) -> RegisterResult => |i| i.register(&req.server_url, &req.password, &req.device_name).await;
    login(req: LoginReq) -> AccountInfo => |i| i.login(&req.server_url, &req.number, &req.password, &req.device_name, req.totp.as_deref()).await;
    totp_setup() -> Value => |i| i.totp_setup().await;
    totp_enable(req: TotpCodeReq) -> () => |i| i.totp_enable(&req.code).await;
    totp_disable(req: TotpCodeReq) -> () => |i| i.totp_disable(&req.code).await;
    recover(req: RecoverReq) -> AccountInfo => |i| i.recover(&req.server_url, &req.number, &req.mnemonic, &req.new_password, &req.device_name).await;
    logout() -> () => |i| i.logout().await;
    get_connection_state() -> ConnState => |i| Ok(i.conn_state.get());
    confirm_mnemonic_saved() -> () => |i| i.confirm_mnemonic_saved();
    link_create(req: LinkCreateReq) -> String => |i| i.link_create(&req.server_url, &req.device_name).await;
    link_claim(req: LinkClaimReq) -> () => |i| i.link_claim(&req.link, &req.device_name).await;
    link_wait() -> AccountInfo => |i| i.link_wait().await;
    link_cancel() -> () => |i| { i.link_cancel(); Ok(()) };
    link_offer() -> String => |i| i.link_offer().await;
    link_offer_wait() -> String => |i| i.link_offer_wait().await;
    link_code() -> Option<String> => |i| Ok(i.link_code());
    link_inspect(req: LinkReq) -> String => |i| i.link_inspect(&req.link).await;
    link_approve(req: LinkApproveReq) -> () => |i| i.link_approve(req.link.as_deref()).await;
    verify_password(req: PasswordReq) -> bool => |i| i.verify_password(&req.password).await;
    unlock(req: PasswordReq) -> () => |i| i.unlock(&req.password).await;
    read_media(req: PathReq) -> String => |i| i.read_media_base64(&req.path).await;
    export_media(req: ExportMediaReq) -> () => |i| i.export_media_file(std::path::Path::new(&req.path), std::path::Path::new(&req.dest)).await;
    protect_local_data(req: PasswordReq) -> () => |i| i.protect_local_data(&req.password).await;
    reset_local_data(req: Option<PasswordReq>) -> () => |i| i.reset_local_data_checked(req.as_ref().map(|r| r.password.as_str())).await;
    get_account_details() -> AccountDetails => |i| i.account_details().await;
    change_password(req: ChangePasswordReq) -> () => |i| i.change_password(&req.old_password, &req.new_password).await;
    reveal_mnemonic(req: PasswordReq) -> Vec<String> => |i| i.reveal_mnemonic(&req.password).await;
    delete_account(req: PasswordReq) -> () => |i| i.delete_account(&req.password).await;
    set_privacy(req: PrivacyPatch) -> PrivacySettings => |i| i.set_privacy(req).await;
    get_dnd() -> DndState => |i| Ok(i.get_dnd());
    // `as` saturates: anything past i64::MAX stays "until turned off".
    set_dnd(req: DndReq) -> DndState => |i| i.set_dnd(req.until as i64).await;
    // Call privacy (engine/call_settings.rs).
    get_call_settings() -> crate::engine::CallSettings => |i| Ok(i.get_call_settings());
    set_call_settings(req: crate::engine::CallSettingsPatch) -> crate::engine::CallSettings => |i| i.set_call_settings(req).await;
    // Media retention period and the media check (engine/retention.rs).
    get_media_retention() -> crate::engine::retention::MediaRetention => |i| Ok(i.get_media_retention());
    set_media_retention(req: MediaRetentionReq) -> crate::engine::retention::MediaRetention => |i| i.set_media_retention(req.days).await;
    // Timer of the conversations we start (engine/default_timer.rs).
    get_default_disappearing() -> crate::engine::DefaultDisappearing => |i| Ok(i.get_default_disappearing());
    set_default_disappearing(req: DefaultDisappearingReq) -> crate::engine::DefaultDisappearing => |i| i.set_default_disappearing(req.seconds).await;
    run_media_check() -> crate::engine::retention::MediaCheck => |i| i.run_media_check().await;
    reminder_shown_at(req: ReminderReq) -> Option<i64> => |i| i.reminder_shown_at(&req.name);
    mark_reminder_shown(req: ReminderReq) -> () => |i| i.mark_reminder_shown(&req.name);
    set_backup_enabled(req: EnabledReq) -> () => |i| i.set_backup_enabled(req.enabled).await;
    request_phone_code(req: PhoneReq) -> () => |i| i.request_phone_code(&req.phone).await;
    verify_phone(req: VerifyPhoneReq) -> () => |i| i.verify_phone(&req.phone, &req.code).await;
    remove_phone() -> () => |i| i.remove_phone().await;
    set_discoverable(req: DiscoverableReq) -> () => |i| i.set_discoverable(req.discoverable).await;
    list_devices() -> Vec<DeviceInfo> => |i| i.list_devices().await;
    unlink_device(req: DeviceIdReq) -> () => |i| i.unlink_device(req.device_id).await;
    rename_device(req: NameReq) -> () => |i| i.rename_device(&req.name).await;
    get_profile() -> Profile => |i| i.get_profile();
    set_profile(req: SetProfileRequest) -> Profile => |i| i.set_profile(req).await;
    list_contacts() -> Vec<Contact> => |i| i.list_contacts();
    get_contact(req: AccountIdReq) -> Contact => |i| i.contact_model(&req.account_id);
    add_contact_by_number(req: AddByNumberReq) -> Contact => |i| i.add_contact_by_number(&req.number, req.name).await;
    rename_contact(req: RenameContactReq) -> Contact => |i| i.rename_contact(&req.account_id, req.name);
    set_blocked(req: BlockReq) -> Contact => |i| i.set_blocked(&req.account_id, req.blocked);
    import_contacts(req: PathReq) -> ContactImportResult => |i| i.import_contacts(&req.path).await;
    get_my_contact_link() -> String => |i| i.my_contact_link();
    add_contact_from_link(req: LinkReq) -> Contact => |i| i.add_contact_from_link(&req.link).await;
    get_username_link() -> Option<UsernameLink> => |i| i.get_username_link().await;
    reset_username_link() -> Option<UsernameLink> => |i| i.reset_username_link().await;
    resolve_username_link(req: LinkReq) -> FoundAccount => |i| i.resolve_username_link(&req.link).await;
    get_safety_number(req: AccountIdReq) -> SafetyNumber => |i| i.safety_number(&req.account_id).await;
    set_verified(req: VerifiedReq) -> Contact => |i| i.set_verified(&req.account_id, req.verified);
    watch_presence(req: AccountIdsReq) -> () => |i| { i.watch_presence(req.account_ids); Ok(()) };
    list_conversations() -> Vec<Conversation> => |i| i.list_conversations();
    get_conversation(req: ConvReq) -> Conversation => |i| i.get_conversation(&req.conversation_id);
    open_direct_conversation(req: AccountIdReq) -> Conversation => |i| i.open_direct_conversation(&req.account_id).await;
    accept_conversation(req: ConvReq) -> Conversation => |i| i.accept_conversation(&req.conversation_id).await;
    set_conversation_settings(req: ConversationSettingsPatch) -> Conversation => |i| i.set_conversation_settings(req).await;
    set_draft(req: DraftReq) -> () => |i| i.set_draft(&req.conversation_id, req.draft);
    delete_conversation(req: ConvReq) -> () => |i| i.delete_conversation(&req.conversation_id);
    clear_conversation(req: ConvReq) -> () => |i| i.clear_conversation(&req.conversation_id);
    clear_group_for_everyone(req: ConvPasswordReq) -> () => |i| {
        i.require_password(&req.password).await?;
        i.clear_group_for_everyone(&req.conversation_id)
    };
    delete_conversation_for_everyone(req: ConvPasswordReq) -> () => |i| {
        i.require_password(&req.password).await?;
        i.delete_conversation_for_everyone(&req.conversation_id)
    };
    wipe_all_conversations(req: PasswordReq) -> () => |i| {
        i.require_password(&req.password).await?;
        i.wipe_all_conversations()
    };
    get_temporary_mode() -> bool => |i| Ok(i.temporary_mode());
    set_temporary_mode(req: EnabledReq) -> () => |i| i.set_temporary_mode(req.enabled);
    get_messages(req: GetMessagesReq) -> Vec<Message> => |i| i.get_messages(&req.conversation_id, req.before, req.limit);
    send_message(req: SendMessageRequest) -> Message => |i| i.send_message(req).await;
    edit_message(req: EditReq) -> Message => |i| i.edit_message(&req.message_id, &req.text, &req.mentions);
    delete_message(req: DeleteMsgReq) -> () => |i| i.delete_message(&req.message_id, req.for_everyone);
    react(req: ReactReq) -> Message => |i| i.react(&req.message_id, req.emoji);
    pin_message(req: PinReq) -> Message => |i| i.pin_message(&req.message_id, req.seconds);
    set_group_invite(req: SetInviteRequest) -> GroupInfo => |i| i.set_group_invite(req).await;
    preview_group_invite(req: LinkReq) -> InvitePreview => |i| i.preview_group_invite(&req.link).await;
    join_group_by_link(req: LinkReq) -> JoinResult => |i| i.join_group_by_link(&req.link).await;
    cancel_join_request(req: GroupIdReq) -> () => |i| i.cancel_join_request(&req.group_id).await;
    group_join_requests(req: ConvReq) -> Vec<JoinRequest> => |i| i.group_join_requests(&req.conversation_id).await;
    approve_join_request(req: ConvAccountReq) -> GroupInfo => |i| i.approve_join_request(&req.conversation_id, &req.account_id).await;
    reject_join_request(req: ConvAccountReq) -> GroupInfo => |i| i.reject_join_request(&req.conversation_id, &req.account_id).await;
    group_bans(req: ConvReq) -> Vec<crate::engine::group_bans::GroupBan> => |i| i.group_bans(&req.conversation_id).await;
    unban_group_member(req: ConvAccountReq) -> GroupInfo => |i| i.unban_group_member(&req.conversation_id, &req.account_id).await;
    star_message(req: StarReq) -> Message => |i| i.star_message(&req.message_id, req.starred);
    pinned_messages(req: ConvReq) -> Vec<Message> => |i| i.pinned_messages(&req.conversation_id);
    starred_messages() -> Vec<Message> => |i| i.starred_messages();
    forward_messages(req: ForwardReq) -> () => |i| i.forward_messages(&req.message_ids, &req.conversation_ids);
    mark_read(req: ConvReq) -> () => |i| i.mark_read(&req.conversation_id);
    send_typing(req: TypingReq) -> () => |i| i.send_typing(&req.conversation_id, req.typing).await;
    call_signal(req: crate::engine::CallSignalRequest) -> () => |i| i.call_signal(req).await;
    record_call(req: crate::engine::RecordCallRequest) -> () => |i| i.record_call(req);
    get_ice_servers() -> Value => |i| i.get_ice_servers().await;
    // Group calls (engine/group_calls.rs).
    group_call_start(req: crate::engine::GroupCallStartRequest) -> crate::engine::GroupCallInfo => |i| i.group_call_start(req).await;
    group_call_join(req: crate::engine::GroupCallJoinRequest) -> crate::engine::GroupCallInfo => |i| i.group_call_join(req).await;
    group_call_leave(req: ConvReq) -> () => |i| i.group_call_leave(&req.conversation_id);
    group_call_decline(req: ConvReq) -> () => |i| i.group_call_decline(&req.conversation_id);
    group_call_heartbeat(req: ConvReq) -> () => |i| i.group_call_heartbeat(&req.conversation_id);
    group_call_state(req: ConvReq) -> Option<crate::engine::GroupCallInfo> => |i| i.group_call_state(&req.conversation_id);
    group_call_signal(req: crate::engine::GroupCallSignalRequest) -> () => |i| i.group_call_signal(req).await;
    retry_message(req: MessageIdReq) -> Message => |i| i.retry_message(&req.message_id);
    get_message_info(req: MessageIdReq) -> MessageInfo => |i| i.message_info(&req.message_id);
    get_edit_history(req: MessageIdReq) -> Vec<EditVersion> => |i| i.get_edit_history(&req.message_id);
    open_view_once(req: MessageIdReq) -> Attachment => |i| i.open_view_once(&req.message_id).await;
    view_once_done(req: MessageIdReq) -> () => |i| i.view_once_done(&req.message_id);
    download_attachment(req: AttachmentReq) -> () => |i| i.download_attachment(&req.message_id, &req.attachment_id).await;
    save_attachment(req: SaveAttachmentReq) -> () => |i| i.save_attachment(&req.message_id, &req.attachment_id, &req.dest_path).await;
    search_messages(req: SearchReq) -> Vec<Message> => |i| i.search_messages(&req.query, req.conversation_id.as_deref(), req.limit);
    list_media(req: MediaReq) -> Vec<Message> => |i| i.list_media(&req.conversation_id, &req.kind, req.limit);
    create_group(req: CreateGroupRequest) -> Conversation => |i| i.create_group(req).await;
    get_group_info(req: ConvReq) -> GroupInfo => |i| i.group_info(&req.conversation_id);
    update_group(req: UpdateGroupRequest) -> GroupInfo => |i| i.update_group(req).await;
    add_group_members(req: MembersReq) -> GroupInfo => |i| i.add_group_members(&req.conversation_id, req.account_ids).await;
    remove_group_member(req: MemberReq) -> GroupInfo => |i| i.remove_group_member(&req.conversation_id, &req.account_id).await;
    set_group_admin(req: AdminReq) -> GroupInfo => |i| i.set_group_admin(&req.conversation_id, &req.account_id, req.admin).await;
    leave_group(req: ConvReq) -> () => |i| i.leave_group(&req.conversation_id).await;
    set_push_token(req: PushTokenReq) -> () => |i| i.set_push_token(&req.provider, &req.token).await;
    // This account's push tag (pushes carry it as `p`).
    push_tag() -> String => |i| i.push_tag();
    // A push arrived: fetch what waits on the server now.
    push_wake() -> () => |i| { i.push_wake(); Ok(()) };
    // Mobile app went to the background (and is not in a call) or came back (engine/net.rs).
    set_background(req: BackgroundReq) -> () => |i| { i.set_background(req.background); Ok(()) };
    // The user is at this device or away since `since` (presence, engine/net.rs).
    set_presence_active(req: PresenceActiveReq) -> () => |i| { i.set_presence_active(req.active, req.since); Ok(()) };
    // Account security (engine/security.rs).
    device_alerts() -> Vec<NewDeviceAlert> => |i| i.device_alerts();
    dismiss_device_alerts() -> () => |i| i.dismiss_device_alerts();
    login_wait_approval() -> AccountInfo => |i| i.login_wait_approval().await;
    login_cancel_approval() -> () => |i| { i.login_cancel_approval(); Ok(()) };
    list_login_requests() -> Vec<LoginRequestInfo> => |i| i.list_login_requests().await;
    answer_login_request(req: AnswerLoginReq) -> () => |i| i.answer_login_request(&req.id, req.approve).await;
    set_login_approval(req: EnabledReq) -> () => |i| i.set_login_approval(req.enabled).await;
    report_spam(req: ReportSpamReq) -> Contact => |i| i.report_spam(&req.account_id, req.block).await;
    // Key transparency (engine/keytrans.rs).
    kt_status() -> KtOverview => |i| i.kt_overview();
    kt_contact_status(req: AccountIdReq) -> KtContactStatus => |i| i.kt_contact_status(&req.account_id).await;
    kt_check_now() -> KtOverview => |i| i.kt_check_now().await;
    kt_dismiss_alerts() -> () => |i| i.kt_dismiss_alerts();
}
